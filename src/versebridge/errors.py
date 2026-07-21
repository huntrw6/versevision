from enum import IntEnum


class ExitCode(IntEnum):
    SUCCESS = 0
    GENERAL_FAILURE = 1
    INVALID_ARGUMENTS = 2
    PARSING_OR_VALIDATION = 3
    BIBLE_PROVIDER = 4
    PROPRESENTER_CONNECTION = 5
    UNSUPPORTED_PROPRESENTER = 6
    PRESENTATION_WRITE = 7
    VERIFICATION_ROLLBACK = 8
    USER_CANCELLATION = 9


class VerseBridgeError(Exception):
    """Base application error."""

    exit_code = ExitCode.GENERAL_FAILURE


class DocumentError(VerseBridgeError):
    pass


class ReferenceValidationError(VerseBridgeError):
    exit_code = ExitCode.PARSING_OR_VALIDATION


class ConfigurationError(VerseBridgeError):
    exit_code = ExitCode.INVALID_ARGUMENTS


class BibleProviderError(VerseBridgeError):
    exit_code = ExitCode.BIBLE_PROVIDER


class ProPresenterConnectionError(VerseBridgeError):
    exit_code = ExitCode.PROPRESENTER_CONNECTION


class UnsupportedProPresenterError(VerseBridgeError):
    exit_code = ExitCode.UNSUPPORTED_PROPRESENTER


class PresentationWriteError(VerseBridgeError):
    exit_code = ExitCode.PRESENTATION_WRITE


class VerificationRollbackError(VerseBridgeError):
    exit_code = ExitCode.VERIFICATION_ROLLBACK
