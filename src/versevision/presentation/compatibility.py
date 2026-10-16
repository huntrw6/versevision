from dataclasses import dataclass


@dataclass(frozen=True)
class CompatibilityPolicy:
    application_major: int = 19
    application_minor: int = 0
    application_patch: int = 1
    # (document major, document build) pairs verified against ProPresenter 19.0.1.
    #   10 / 26200 : Windows spike (2026-07-18)
    #   15 / ""    : macOS 15.7.9 (JPL Graphics iMac, document version 15.7.9)
    document_versions: tuple[tuple[int, str], ...] = ((10, "26200"), (15, ""))

    def validate(self, presentation) -> None:
        app = presentation.application_info.application_version
        document = presentation.application_info.platform_version
        if (app.major_version, app.minor_version, app.patch_version) != (
            self.application_major,
            self.application_minor,
            self.application_patch,
        ):
            raise ValueError(
                f"Unsupported ProPresenter version {app.major_version}.{app.minor_version}.{app.patch_version}"
            )
        if (document.major_version, document.build) not in self.document_versions:
            raise ValueError(
                f"Unsupported ProPresenter document version {document.major_version}/{document.build}"
            )
