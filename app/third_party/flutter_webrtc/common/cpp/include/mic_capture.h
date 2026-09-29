#ifndef FLUTTER_WEBRTC_MIC_CAPTURE_H_
#define FLUTTER_WEBRTC_MIC_CAPTURE_H_

// botato: microphone capture that doesn't go through libwebrtc's audio device module.
//
// libwebrtc's Windows ADM only offers the endpoint a few fixed PCM formats (16 bit, 1-2
// channels); some laptop mic arrays (Intel Smart Sound, 4 channels / 32 bit) accept none of
// them, and the track then carries silence while every other app hears the mic. This capture
// opens the endpoint through WASAPI in a format it takes (Windows converts, or we convert the
// mix format ourselves) and feeds 10 ms frames into a custom audio source.
//
// Custom sources skip WebRTC's audio processing (AEC/NS/AGC); with `communications` the
// stream asks Windows for the communications signal-processing mode (the driver's own echo
// cancellation and noise suppression, where it has them).

#include <string>

#include "rtc_audio_source.h"
#include "rtc_types.h"

namespace flutter_webrtc_plugin {

using namespace libwebrtc;

#ifdef _WIN32
// Starts capturing |endpoint_id| (a WASAPI endpoint ID, as libwebrtc reports it for a
// recording device; empty = the default input) into |source| for |track_id|. Returns an empty
// string and fills |format| on success, otherwise the reason it failed.
std::string StartMicCapture(const std::string& track_id,
                            scoped_refptr<RTCAudioSource> source,
                            const std::string& endpoint_id, int channels,
                            bool communications, std::string& format);
void StopMicCapture(const std::string& track_id);
#else
inline std::string StartMicCapture(const std::string&,
                                   scoped_refptr<RTCAudioSource>,
                                   const std::string&, int, bool,
                                   std::string&) {
  return "not supported on this platform";
}
inline void StopMicCapture(const std::string&) {}
#endif

}  // namespace flutter_webrtc_plugin

#endif  // FLUTTER_WEBRTC_MIC_CAPTURE_H_
