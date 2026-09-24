// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint, type=warning, deprecated_member_use, deprecated_member_use_from_same_package
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'client.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$ClientEvent {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ClientEvent);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'ClientEvent()';
}


}

/// @nodoc
class $ClientEventCopyWith<$Res>  {
$ClientEventCopyWith(ClientEvent _, $Res Function(ClientEvent) __);
}


/// Adds pattern-matching-related methods to [ClientEvent].
extension ClientEventPatterns on ClientEvent {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( ClientEvent_Connecting value)?  connecting,TResult Function( ClientEvent_Online value)?  online,TResult Function( ClientEvent_Offline value)?  offline,TResult Function( ClientEvent_ContactsChanged value)?  contactsChanged,TResult Function( ClientEvent_Signal value)?  signal,TResult Function( ClientEvent_Signaled value)?  signaled,TResult Function( ClientEvent_Turn value)?  turn,TResult Function( ClientEvent_Error value)?  error,required TResult orElse(),}){
final _that = this;
switch (_that) {
case ClientEvent_Connecting() when connecting != null:
return connecting(_that);case ClientEvent_Online() when online != null:
return online(_that);case ClientEvent_Offline() when offline != null:
return offline(_that);case ClientEvent_ContactsChanged() when contactsChanged != null:
return contactsChanged(_that);case ClientEvent_Signal() when signal != null:
return signal(_that);case ClientEvent_Signaled() when signaled != null:
return signaled(_that);case ClientEvent_Turn() when turn != null:
return turn(_that);case ClientEvent_Error() when error != null:
return error(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( ClientEvent_Connecting value)  connecting,required TResult Function( ClientEvent_Online value)  online,required TResult Function( ClientEvent_Offline value)  offline,required TResult Function( ClientEvent_ContactsChanged value)  contactsChanged,required TResult Function( ClientEvent_Signal value)  signal,required TResult Function( ClientEvent_Signaled value)  signaled,required TResult Function( ClientEvent_Turn value)  turn,required TResult Function( ClientEvent_Error value)  error,}){
final _that = this;
switch (_that) {
case ClientEvent_Connecting():
return connecting(_that);case ClientEvent_Online():
return online(_that);case ClientEvent_Offline():
return offline(_that);case ClientEvent_ContactsChanged():
return contactsChanged(_that);case ClientEvent_Signal():
return signal(_that);case ClientEvent_Signaled():
return signaled(_that);case ClientEvent_Turn():
return turn(_that);case ClientEvent_Error():
return error(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( ClientEvent_Connecting value)?  connecting,TResult? Function( ClientEvent_Online value)?  online,TResult? Function( ClientEvent_Offline value)?  offline,TResult? Function( ClientEvent_ContactsChanged value)?  contactsChanged,TResult? Function( ClientEvent_Signal value)?  signal,TResult? Function( ClientEvent_Signaled value)?  signaled,TResult? Function( ClientEvent_Turn value)?  turn,TResult? Function( ClientEvent_Error value)?  error,}){
final _that = this;
switch (_that) {
case ClientEvent_Connecting() when connecting != null:
return connecting(_that);case ClientEvent_Online() when online != null:
return online(_that);case ClientEvent_Offline() when offline != null:
return offline(_that);case ClientEvent_ContactsChanged() when contactsChanged != null:
return contactsChanged(_that);case ClientEvent_Signal() when signal != null:
return signal(_that);case ClientEvent_Signaled() when signaled != null:
return signaled(_that);case ClientEvent_Turn() when turn != null:
return turn(_that);case ClientEvent_Error() when error != null:
return error(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function( String server)?  connecting,TResult Function( String server)?  online,TResult Function( String reason)?  offline,TResult Function()?  contactsChanged,TResult Function( String fromUser,  String fromDevice,  String payload)?  signal,TResult Function( String toDevice,  bool online)?  signaled,TResult Function( List<String> urls,  String username,  String credential)?  turn,TResult Function( String message)?  error,required TResult orElse(),}) {final _that = this;
switch (_that) {
case ClientEvent_Connecting() when connecting != null:
return connecting(_that.server);case ClientEvent_Online() when online != null:
return online(_that.server);case ClientEvent_Offline() when offline != null:
return offline(_that.reason);case ClientEvent_ContactsChanged() when contactsChanged != null:
return contactsChanged();case ClientEvent_Signal() when signal != null:
return signal(_that.fromUser,_that.fromDevice,_that.payload);case ClientEvent_Signaled() when signaled != null:
return signaled(_that.toDevice,_that.online);case ClientEvent_Turn() when turn != null:
return turn(_that.urls,_that.username,_that.credential);case ClientEvent_Error() when error != null:
return error(_that.message);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function( String server)  connecting,required TResult Function( String server)  online,required TResult Function( String reason)  offline,required TResult Function()  contactsChanged,required TResult Function( String fromUser,  String fromDevice,  String payload)  signal,required TResult Function( String toDevice,  bool online)  signaled,required TResult Function( List<String> urls,  String username,  String credential)  turn,required TResult Function( String message)  error,}) {final _that = this;
switch (_that) {
case ClientEvent_Connecting():
return connecting(_that.server);case ClientEvent_Online():
return online(_that.server);case ClientEvent_Offline():
return offline(_that.reason);case ClientEvent_ContactsChanged():
return contactsChanged();case ClientEvent_Signal():
return signal(_that.fromUser,_that.fromDevice,_that.payload);case ClientEvent_Signaled():
return signaled(_that.toDevice,_that.online);case ClientEvent_Turn():
return turn(_that.urls,_that.username,_that.credential);case ClientEvent_Error():
return error(_that.message);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function( String server)?  connecting,TResult? Function( String server)?  online,TResult? Function( String reason)?  offline,TResult? Function()?  contactsChanged,TResult? Function( String fromUser,  String fromDevice,  String payload)?  signal,TResult? Function( String toDevice,  bool online)?  signaled,TResult? Function( List<String> urls,  String username,  String credential)?  turn,TResult? Function( String message)?  error,}) {final _that = this;
switch (_that) {
case ClientEvent_Connecting() when connecting != null:
return connecting(_that.server);case ClientEvent_Online() when online != null:
return online(_that.server);case ClientEvent_Offline() when offline != null:
return offline(_that.reason);case ClientEvent_ContactsChanged() when contactsChanged != null:
return contactsChanged();case ClientEvent_Signal() when signal != null:
return signal(_that.fromUser,_that.fromDevice,_that.payload);case ClientEvent_Signaled() when signaled != null:
return signaled(_that.toDevice,_that.online);case ClientEvent_Turn() when turn != null:
return turn(_that.urls,_that.username,_that.credential);case ClientEvent_Error() when error != null:
return error(_that.message);case _:
  return null;

}
}

}

/// @nodoc


class ClientEvent_Connecting extends ClientEvent {
  const ClientEvent_Connecting({required this.server}): super._();
  

 final  String server;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ClientEvent_ConnectingCopyWith<ClientEvent_Connecting> get copyWith => _$ClientEvent_ConnectingCopyWithImpl<ClientEvent_Connecting>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ClientEvent_Connecting&&(identical(other.server, server) || other.server == server));
}


@override
int get hashCode {
    return Object.hash(runtimeType,server);
}

@override
String toString() {
    return 'ClientEvent.connecting(server: $server)';
}


}

/// @nodoc
abstract mixin class $ClientEvent_ConnectingCopyWith<$Res> implements $ClientEventCopyWith<$Res> {
  factory $ClientEvent_ConnectingCopyWith(ClientEvent_Connecting value, $Res Function(ClientEvent_Connecting) _then) = _$ClientEvent_ConnectingCopyWithImpl;
@useResult
$Res call({
 String server
});




}
/// @nodoc
class _$ClientEvent_ConnectingCopyWithImpl<$Res>
    implements $ClientEvent_ConnectingCopyWith<$Res> {
  _$ClientEvent_ConnectingCopyWithImpl(this._self, this._then);

  final ClientEvent_Connecting _self;
  final $Res Function(ClientEvent_Connecting) _then;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? server = null,}) {
  return _then(ClientEvent_Connecting(
server: null == server ? _self.server : server // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ClientEvent_Online extends ClientEvent {
  const ClientEvent_Online({required this.server}): super._();
  

 final  String server;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ClientEvent_OnlineCopyWith<ClientEvent_Online> get copyWith => _$ClientEvent_OnlineCopyWithImpl<ClientEvent_Online>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ClientEvent_Online&&(identical(other.server, server) || other.server == server));
}


@override
int get hashCode {
    return Object.hash(runtimeType,server);
}

@override
String toString() {
    return 'ClientEvent.online(server: $server)';
}


}

/// @nodoc
abstract mixin class $ClientEvent_OnlineCopyWith<$Res> implements $ClientEventCopyWith<$Res> {
  factory $ClientEvent_OnlineCopyWith(ClientEvent_Online value, $Res Function(ClientEvent_Online) _then) = _$ClientEvent_OnlineCopyWithImpl;
@useResult
$Res call({
 String server
});




}
/// @nodoc
class _$ClientEvent_OnlineCopyWithImpl<$Res>
    implements $ClientEvent_OnlineCopyWith<$Res> {
  _$ClientEvent_OnlineCopyWithImpl(this._self, this._then);

  final ClientEvent_Online _self;
  final $Res Function(ClientEvent_Online) _then;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? server = null,}) {
  return _then(ClientEvent_Online(
server: null == server ? _self.server : server // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ClientEvent_Offline extends ClientEvent {
  const ClientEvent_Offline({required this.reason}): super._();
  

 final  String reason;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ClientEvent_OfflineCopyWith<ClientEvent_Offline> get copyWith => _$ClientEvent_OfflineCopyWithImpl<ClientEvent_Offline>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ClientEvent_Offline&&(identical(other.reason, reason) || other.reason == reason));
}


@override
int get hashCode {
    return Object.hash(runtimeType,reason);
}

@override
String toString() {
    return 'ClientEvent.offline(reason: $reason)';
}


}

/// @nodoc
abstract mixin class $ClientEvent_OfflineCopyWith<$Res> implements $ClientEventCopyWith<$Res> {
  factory $ClientEvent_OfflineCopyWith(ClientEvent_Offline value, $Res Function(ClientEvent_Offline) _then) = _$ClientEvent_OfflineCopyWithImpl;
@useResult
$Res call({
 String reason
});




}
/// @nodoc
class _$ClientEvent_OfflineCopyWithImpl<$Res>
    implements $ClientEvent_OfflineCopyWith<$Res> {
  _$ClientEvent_OfflineCopyWithImpl(this._self, this._then);

  final ClientEvent_Offline _self;
  final $Res Function(ClientEvent_Offline) _then;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? reason = null,}) {
  return _then(ClientEvent_Offline(
reason: null == reason ? _self.reason : reason // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ClientEvent_ContactsChanged extends ClientEvent {
  const ClientEvent_ContactsChanged(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ClientEvent_ContactsChanged);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'ClientEvent.contactsChanged()';
}


}




/// @nodoc


class ClientEvent_Signal extends ClientEvent {
  const ClientEvent_Signal({required this.fromUser, required this.fromDevice, required this.payload}): super._();
  

 final  String fromUser;
 final  String fromDevice;
 final  String payload;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ClientEvent_SignalCopyWith<ClientEvent_Signal> get copyWith => _$ClientEvent_SignalCopyWithImpl<ClientEvent_Signal>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ClientEvent_Signal&&(identical(other.fromUser, fromUser) || other.fromUser == fromUser)&&(identical(other.fromDevice, fromDevice) || other.fromDevice == fromDevice)&&(identical(other.payload, payload) || other.payload == payload));
}


@override
int get hashCode {
    return Object.hash(runtimeType,fromUser,fromDevice,payload);
}

@override
String toString() {
    return 'ClientEvent.signal(fromUser: $fromUser, fromDevice: $fromDevice, payload: $payload)';
}


}

/// @nodoc
abstract mixin class $ClientEvent_SignalCopyWith<$Res> implements $ClientEventCopyWith<$Res> {
  factory $ClientEvent_SignalCopyWith(ClientEvent_Signal value, $Res Function(ClientEvent_Signal) _then) = _$ClientEvent_SignalCopyWithImpl;
@useResult
$Res call({
 String fromUser, String fromDevice, String payload
});




}
/// @nodoc
class _$ClientEvent_SignalCopyWithImpl<$Res>
    implements $ClientEvent_SignalCopyWith<$Res> {
  _$ClientEvent_SignalCopyWithImpl(this._self, this._then);

  final ClientEvent_Signal _self;
  final $Res Function(ClientEvent_Signal) _then;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? fromUser = null,Object? fromDevice = null,Object? payload = null,}) {
  return _then(ClientEvent_Signal(
fromUser: null == fromUser ? _self.fromUser : fromUser // ignore: cast_nullable_to_non_nullable
as String,fromDevice: null == fromDevice ? _self.fromDevice : fromDevice // ignore: cast_nullable_to_non_nullable
as String,payload: null == payload ? _self.payload : payload // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ClientEvent_Signaled extends ClientEvent {
  const ClientEvent_Signaled({required this.toDevice, required this.online}): super._();
  

 final  String toDevice;
 final  bool online;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ClientEvent_SignaledCopyWith<ClientEvent_Signaled> get copyWith => _$ClientEvent_SignaledCopyWithImpl<ClientEvent_Signaled>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ClientEvent_Signaled&&(identical(other.toDevice, toDevice) || other.toDevice == toDevice)&&(identical(other.online, online) || other.online == online));
}


@override
int get hashCode {
    return Object.hash(runtimeType,toDevice,online);
}

@override
String toString() {
    return 'ClientEvent.signaled(toDevice: $toDevice, online: $online)';
}


}

/// @nodoc
abstract mixin class $ClientEvent_SignaledCopyWith<$Res> implements $ClientEventCopyWith<$Res> {
  factory $ClientEvent_SignaledCopyWith(ClientEvent_Signaled value, $Res Function(ClientEvent_Signaled) _then) = _$ClientEvent_SignaledCopyWithImpl;
@useResult
$Res call({
 String toDevice, bool online
});




}
/// @nodoc
class _$ClientEvent_SignaledCopyWithImpl<$Res>
    implements $ClientEvent_SignaledCopyWith<$Res> {
  _$ClientEvent_SignaledCopyWithImpl(this._self, this._then);

  final ClientEvent_Signaled _self;
  final $Res Function(ClientEvent_Signaled) _then;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? toDevice = null,Object? online = null,}) {
  return _then(ClientEvent_Signaled(
toDevice: null == toDevice ? _self.toDevice : toDevice // ignore: cast_nullable_to_non_nullable
as String,online: null == online ? _self.online : online // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class ClientEvent_Turn extends ClientEvent {
  const ClientEvent_Turn({required  List<String> urls, required this.username, required this.credential}): _urls = urls,super._();
  

 final  List<String> _urls;
 List<String> get urls {
  if (_urls is EqualUnmodifiableListView) return _urls;
  // ignore: implicit_dynamic_type
  return EqualUnmodifiableListView(_urls);
}

 final  String username;
 final  String credential;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ClientEvent_TurnCopyWith<ClientEvent_Turn> get copyWith => _$ClientEvent_TurnCopyWithImpl<ClientEvent_Turn>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ClientEvent_Turn&&const DeepCollectionEquality().equals(other.urls, _urls)&&(identical(other.username, username) || other.username == username)&&(identical(other.credential, credential) || other.credential == credential));
}


@override
int get hashCode {
    return Object.hash(runtimeType,const DeepCollectionEquality().hash(_urls),username,credential);
}

@override
String toString() {
    return 'ClientEvent.turn(urls: $urls, username: $username, credential: $credential)';
}


}

/// @nodoc
abstract mixin class $ClientEvent_TurnCopyWith<$Res> implements $ClientEventCopyWith<$Res> {
  factory $ClientEvent_TurnCopyWith(ClientEvent_Turn value, $Res Function(ClientEvent_Turn) _then) = _$ClientEvent_TurnCopyWithImpl;
@useResult
$Res call({
 List<String> urls, String username, String credential
});




}
/// @nodoc
class _$ClientEvent_TurnCopyWithImpl<$Res>
    implements $ClientEvent_TurnCopyWith<$Res> {
  _$ClientEvent_TurnCopyWithImpl(this._self, this._then);

  final ClientEvent_Turn _self;
  final $Res Function(ClientEvent_Turn) _then;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? urls = null,Object? username = null,Object? credential = null,}) {
  return _then(ClientEvent_Turn(
urls: null == urls ? _self._urls : urls // ignore: cast_nullable_to_non_nullable
as List<String>,username: null == username ? _self.username : username // ignore: cast_nullable_to_non_nullable
as String,credential: null == credential ? _self.credential : credential // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class ClientEvent_Error extends ClientEvent {
  const ClientEvent_Error({required this.message}): super._();
  

 final  String message;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$ClientEvent_ErrorCopyWith<ClientEvent_Error> get copyWith => _$ClientEvent_ErrorCopyWithImpl<ClientEvent_Error>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is ClientEvent_Error&&(identical(other.message, message) || other.message == message));
}


@override
int get hashCode {
    return Object.hash(runtimeType,message);
}

@override
String toString() {
    return 'ClientEvent.error(message: $message)';
}


}

/// @nodoc
abstract mixin class $ClientEvent_ErrorCopyWith<$Res> implements $ClientEventCopyWith<$Res> {
  factory $ClientEvent_ErrorCopyWith(ClientEvent_Error value, $Res Function(ClientEvent_Error) _then) = _$ClientEvent_ErrorCopyWithImpl;
@useResult
$Res call({
 String message
});




}
/// @nodoc
class _$ClientEvent_ErrorCopyWithImpl<$Res>
    implements $ClientEvent_ErrorCopyWith<$Res> {
  _$ClientEvent_ErrorCopyWithImpl(this._self, this._then);

  final ClientEvent_Error _self;
  final $Res Function(ClientEvent_Error) _then;

/// Create a copy of ClientEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? message = null,}) {
  return _then(ClientEvent_Error(
message: null == message ? _self.message : message // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

// dart format on
