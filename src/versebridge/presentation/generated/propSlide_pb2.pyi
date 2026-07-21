import customOptions_pb2 as _customOptions_pb2
import slide_pb2 as _slide_pb2
import effects_pb2 as _effects_pb2
from google.protobuf import descriptor as _descriptor
from google.protobuf import message as _message
from collections.abc import Mapping as _Mapping
from typing import ClassVar as _ClassVar, Optional as _Optional, Union as _Union

DESCRIPTOR: _descriptor.FileDescriptor

class PropSlide(_message.Message):
    __slots__ = ("base_slide", "transition", "auto_clear_enabled", "auto_clear_duration")
    BASE_SLIDE_FIELD_NUMBER: _ClassVar[int]
    TRANSITION_FIELD_NUMBER: _ClassVar[int]
    AUTO_CLEAR_ENABLED_FIELD_NUMBER: _ClassVar[int]
    AUTO_CLEAR_DURATION_FIELD_NUMBER: _ClassVar[int]
    base_slide: _slide_pb2.Slide
    transition: _effects_pb2.Transition
    auto_clear_enabled: bool
    auto_clear_duration: float
    def __init__(self, base_slide: _Optional[_Union[_slide_pb2.Slide, _Mapping]] = ..., transition: _Optional[_Union[_effects_pb2.Transition, _Mapping]] = ..., auto_clear_enabled: _Optional[bool] = ..., auto_clear_duration: _Optional[float] = ...) -> None: ...
