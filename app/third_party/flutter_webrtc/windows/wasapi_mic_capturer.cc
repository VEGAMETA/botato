#ifdef _WIN32

// botato: WASAPI microphone capture into a custom audio source (see mic_capture.h).

#include <audioclient.h>
#include <avrt.h>
#include <mmdeviceapi.h>
#include <windows.h>

#include <atomic>
#include <cstdio>
#include <cstring>
#include <future>
#include <map>
#include <memory>
#include <mutex>
#include <thread>
#include <vector>

#include "mic_capture.h"

namespace flutter_webrtc_plugin {

namespace {

const GUID kSubtypeFloat = {0x00000003, 0x0000, 0x0010,
                            {0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71}};

std::wstring Widen(const std::string& s) {
  if (s.empty()) return std::wstring();
  int n = MultiByteToWideChar(CP_UTF8, 0, s.data(), (int)s.size(), nullptr, 0);
  std::wstring w(n, L'\0');
  MultiByteToWideChar(CP_UTF8, 0, s.data(), (int)s.size(), &w[0], n);
  return w;
}

std::string Hex(HRESULT hr) {
  char buf[16];
  snprintf(buf, sizeof(buf), "0x%08lx", static_cast<unsigned long>(hr));
  return buf;
}

template <typename T>
void SafeRelease(T*& p) {
  if (p) {
    p->Release();
    p = nullptr;
  }
}

class WasapiMicCapturer {
 public:
  WasapiMicCapturer(scoped_refptr<RTCAudioSource> source, std::string endpoint,
                    int channels, bool communications)
      : source_(source),
        endpoint_(std::move(endpoint)),
        out_channels_(channels == 2 ? 2 : 1),
        communications_(communications) {}

  ~WasapiMicCapturer() { Stop(); }

  // Opens the endpoint on the capture thread (MTA) and waits for the outcome.
  std::string Start(std::string& format) {
    event_ = CreateEvent(nullptr, FALSE, FALSE, nullptr);
    if (!event_) return "CreateEvent failed";
    running_ = true;
    std::promise<std::string> opened;
    auto result = opened.get_future();
    thread_ = std::thread([this, &opened] { Run(opened); });
    if (result.wait_for(std::chrono::seconds(5)) != std::future_status::ready) {
      Stop();
      return "timed out opening the microphone";
    }
    std::string error = result.get();
    if (!error.empty()) {
      Stop();
      return error;
    }
    format = format_;
    return "";
  }

  void Stop() {
    running_ = false;
    if (event_) SetEvent(event_);
    if (thread_.joinable()) thread_.join();
    if (event_) {
      CloseHandle(event_);
      event_ = nullptr;
    }
  }

 private:
  // Everything COM lives and dies on this thread.
  void Run(std::promise<std::string>& opened) {
    CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    IMMDeviceEnumerator* enumerator = nullptr;
    IMMDevice* device = nullptr;
    IAudioClient* client = nullptr;
    IAudioCaptureClient* capture = nullptr;
    WAVEFORMATEX* mix = nullptr;

    std::string error = Open(enumerator, device, client, capture, mix);
    bool ok = error.empty();
    opened.set_value(error);  // |opened| is gone after this on failure paths

    if (ok) {
      DWORD task_index = 0;
      HANDLE task = AvSetMmThreadCharacteristicsW(L"Audio", &task_index);
      Loop(capture);
      client->Stop();
      if (task) AvRevertMmThreadCharacteristics(task);
    }

    SafeRelease(capture);
    SafeRelease(client);
    SafeRelease(device);
    SafeRelease(enumerator);
    if (mix) CoTaskMemFree(mix);
    CoUninitialize();
  }

  std::string Open(IMMDeviceEnumerator*& enumerator, IMMDevice*& device,
                   IAudioClient*& client, IAudioCaptureClient*& capture,
                   WAVEFORMATEX*& mix) {
    HRESULT hr = CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr,
                                  CLSCTX_ALL, IID_PPV_ARGS(&enumerator));
    if (FAILED(hr)) return "MMDeviceEnumerator " + Hex(hr);
    if (!endpoint_.empty()) {
      hr = enumerator->GetDevice(Widen(endpoint_).c_str(), &device);
    }
    if (endpoint_.empty() || FAILED(hr)) {
      hr = enumerator->GetDefaultAudioEndpoint(eCapture, eConsole, &device);
    }
    if (FAILED(hr)) return "no capture endpoint " + Hex(hr);

    hr = device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr,
                          reinterpret_cast<void**>(&client));
    if (FAILED(hr)) return "Activate " + Hex(hr);
    hr = client->GetMixFormat(&mix);
    if (FAILED(hr) || !mix) return "GetMixFormat " + Hex(hr);

    // 1) Our format, Windows converts (any channel count, depth and rate).
    // 2) The endpoint's own mix format, converted here.
    // Each with the communications category first when asked, then without.
    WAVEFORMATEX want = {};
    want.wFormatTag = WAVE_FORMAT_PCM;
    want.nChannels = static_cast<WORD>(out_channels_);
    want.nSamplesPerSec = 48000;
    want.wBitsPerSample = 16;
    want.nBlockAlign = static_cast<WORD>(want.nChannels * 2);
    want.nAvgBytesPerSec = want.nSamplesPerSec * want.nBlockAlign;

    const DWORD base = AUDCLNT_STREAMFLAGS_EVENTCALLBACK |
                       AUDCLNT_STREAMFLAGS_NOPERSIST;
    const DWORD convert = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM |
                          AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    std::string last;
    for (int comm = communications_ ? 1 : 0; comm >= 0; --comm) {
      for (int own = 1; own >= 0; --own) {
        if (!client) {
          hr = device->Activate(__uuidof(IAudioClient), CLSCTX_ALL, nullptr,
                                reinterpret_cast<void**>(&client));
          if (FAILED(hr)) return "Activate " + Hex(hr);
        }
        if (comm) SetCategory(client, true);
        const WAVEFORMATEX* fmt = own ? &want : mix;
        // 20 ms buffer: low latency, and room for a late wakeup.
        hr = client->Initialize(AUDCLNT_SHAREMODE_SHARED,
                                base | (own ? convert : 0), 200000, 0, fmt,
                                nullptr);
        if (SUCCEEDED(hr)) {
          if (!SetupFormat(fmt)) {
            return "unsupported format " + Describe(fmt);
          }
          hr = client->SetEventHandle(event_);
          if (FAILED(hr)) return "SetEventHandle " + Hex(hr);
          hr = client->GetService(IID_PPV_ARGS(&capture));
          if (FAILED(hr)) return "GetService " + Hex(hr);
          hr = client->Start();
          if (FAILED(hr)) return "Start " + Hex(hr);
          format_ = std::string(own ? "converted by Windows from " : "own conversion of ") +
                    Describe(mix) + (comm ? ", communications" : "");
          return "";
        }
        last = std::string(own ? "converted" : "mix format") +
               (comm ? " comm" : "") + " " + Hex(hr);
        SafeRelease(client);  // an IAudioClient initializes only once
      }
    }
    return "Initialize failed (" + last + ", endpoint " + Describe(mix) + ")";
  }

  static void SetCategory(IAudioClient* client, bool communications) {
    IAudioClient2* client2 = nullptr;
    if (FAILED(client->QueryInterface(IID_PPV_ARGS(&client2)))) return;
    AudioClientProperties props = {};
    props.cbSize = sizeof(props);
    props.bIsOffload = FALSE;
    props.eCategory =
        communications ? AudioCategory_Communications : AudioCategory_Other;
    client2->SetClientProperties(&props);
    client2->Release();
  }

  static std::string Describe(const WAVEFORMATEX* f) {
    bool is_float = f->wFormatTag == WAVE_FORMAT_IEEE_FLOAT;
    if (f->wFormatTag == WAVE_FORMAT_EXTENSIBLE) {
      is_float = reinterpret_cast<const WAVEFORMATEXTENSIBLE*>(f)->SubFormat ==
                 kSubtypeFloat;
    }
    char buf[96];
    snprintf(buf, sizeof(buf), "%lu Hz, %u ch, %u-bit %s",
             static_cast<unsigned long>(f->nSamplesPerSec), f->nChannels,
             f->wBitsPerSample, is_float ? "float" : "int");
    return buf;
  }

  bool SetupFormat(const WAVEFORMATEX* f) {
    in_channels_ = f->nChannels;
    bytes_per_sample_ = f->wBitsPerSample / 8;
    rate_ = static_cast<int>(f->nSamplesPerSec);
    is_float_ = f->wFormatTag == WAVE_FORMAT_IEEE_FLOAT;
    int valid_bits = f->wBitsPerSample;
    if (f->wFormatTag == WAVE_FORMAT_EXTENSIBLE) {
      auto* ext = reinterpret_cast<const WAVEFORMATEXTENSIBLE*>(f);
      is_float_ = ext->SubFormat == kSubtypeFloat;
      if (ext->Samples.wValidBitsPerSample) valid_bits = ext->Samples.wValidBitsPerSample;
    }
    (void)valid_bits;  // container alignment is what matters for reading
    if (in_channels_ == 0 || rate_ % 100 != 0) return false;
    if (is_float_) return bytes_per_sample_ == 4;
    return bytes_per_sample_ == 2 || bytes_per_sample_ == 3 ||
           bytes_per_sample_ == 4;
  }

  // One sample as a float in [-1, 1].
  float Sample(const BYTE* p) const {
    if (is_float_) {
      float v;
      std::memcpy(&v, p, 4);
      return v;
    }
    switch (bytes_per_sample_) {
      case 2: {
        int16_t v;
        std::memcpy(&v, p, 2);
        return v / 32768.0f;
      }
      case 3: {
        int32_t v = (p[0] << 8) | (p[1] << 16) | (p[2] << 24);
        return v / 2147483648.0f;
      }
      default: {
        int32_t v;
        std::memcpy(&v, p, 4);
        return v / 2147483648.0f;
      }
    }
  }

  static int16_t ToInt16(float v) {
    if (v > 1.0f) v = 1.0f;
    if (v < -1.0f) v = -1.0f;
    return static_cast<int16_t>(v * 32767.0f);
  }

  // Appends |frames| of input to |pending_| as int16 with |out_channels_|: a stereo output
  // takes the first two channels, a mono one averages all of them (mic arrays).
  void Convert(const BYTE* data, UINT32 frames, bool silent) {
    const size_t frame_bytes = in_channels_ * bytes_per_sample_;
    const size_t start = pending_.size();
    pending_.resize(start + frames * out_channels_);
    int16_t* out = pending_.data() + start;
    if (silent || !data) {
      std::memset(out, 0, frames * out_channels_ * sizeof(int16_t));
      return;
    }
    for (UINT32 i = 0; i < frames; ++i) {
      const BYTE* f = data + i * frame_bytes;
      if (out_channels_ == 2) {
        const float l = Sample(f);
        const float r = in_channels_ > 1 ? Sample(f + bytes_per_sample_) : l;
        out[i * 2] = ToInt16(l);
        out[i * 2 + 1] = ToInt16(r);
      } else {
        float sum = 0;
        for (size_t c = 0; c < in_channels_; ++c) {
          sum += Sample(f + c * bytes_per_sample_);
        }
        out[i] = ToInt16(sum / in_channels_);
      }
    }
  }

  // Delivers whole 10 ms frames from this one thread, as they come.
  void Loop(IAudioCaptureClient* capture) {
    const size_t chunk = static_cast<size_t>(rate_ / 100);
    while (running_) {
      DWORD w = WaitForSingleObject(event_, 200);
      if (!running_) break;
      if (w == WAIT_FAILED) break;
      UINT32 packet = 0;
      HRESULT hr;
      while (SUCCEEDED(hr = capture->GetNextPacketSize(&packet)) && packet > 0) {
        BYTE* data = nullptr;
        UINT32 frames = 0;
        DWORD flags = 0;
        hr = capture->GetBuffer(&data, &frames, &flags, nullptr, nullptr);
        if (FAILED(hr)) break;
        Convert(data, frames, (flags & AUDCLNT_BUFFERFLAGS_SILENT) != 0);
        capture->ReleaseBuffer(frames);
      }
      if (hr == AUDCLNT_E_DEVICE_INVALIDATED) break;  // unplugged
      size_t offset = 0;
      while (pending_.size() - offset >= chunk * out_channels_) {
        source_->CaptureFrame(pending_.data() + offset, 16, rate_,
                              out_channels_, chunk);
        offset += chunk * out_channels_;
      }
      pending_.erase(pending_.begin(), pending_.begin() + offset);
    }
  }

  scoped_refptr<RTCAudioSource> source_;
  std::string endpoint_;
  size_t out_channels_;
  bool communications_;
  std::atomic<bool> running_{false};
  HANDLE event_ = nullptr;
  std::thread thread_;
  std::string format_;

  size_t in_channels_ = 0;
  size_t bytes_per_sample_ = 0;
  int rate_ = 48000;
  bool is_float_ = false;
  std::vector<int16_t> pending_;
};

// Touched from the platform thread only (method calls), like the plugin's other maps.
std::map<std::string, std::unique_ptr<WasapiMicCapturer>>& Capturers() {
  static std::map<std::string, std::unique_ptr<WasapiMicCapturer>> map;
  return map;
}

}  // namespace

std::string StartMicCapture(const std::string& track_id,
                            scoped_refptr<RTCAudioSource> source,
                            const std::string& endpoint_id, int channels,
                            bool communications, std::string& format) {
  StopMicCapture(track_id);
  auto capturer = std::make_unique<WasapiMicCapturer>(source, endpoint_id,
                                                      channels, communications);
  std::string error = capturer->Start(format);
  if (!error.empty()) return error;
  Capturers()[track_id] = std::move(capturer);
  return "";
}

void StopMicCapture(const std::string& track_id) {
  auto& map = Capturers();
  auto it = map.find(track_id);
  if (it == map.end()) return;
  it->second->Stop();
  map.erase(it);
}

}  // namespace flutter_webrtc_plugin

#endif  // _WIN32
