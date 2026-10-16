from google.protobuf.internal import enum_type_wrapper as _enum_type_wrapper
from google.protobuf import descriptor as _descriptor
from typing import ClassVar as _ClassVar

DESCRIPTOR: _descriptor.FileDescriptor

class AlphaType(int, metaclass=_enum_type_wrapper.EnumTypeWrapper):
    __slots__ = ()
    ALPHA_TYPE_UNKNOWN: _ClassVar[AlphaType]
    ALPHA_TYPE_STRAIGHT: _ClassVar[AlphaType]
    ALPHA_TYPE_PREMULTIPLIED: _ClassVar[AlphaType]
ALPHA_TYPE_UNKNOWN: AlphaType
ALPHA_TYPE_STRAIGHT: AlphaType
ALPHA_TYPE_PREMULTIPLIED: AlphaType
