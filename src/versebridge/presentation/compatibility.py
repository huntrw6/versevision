from dataclasses import dataclass


@dataclass(frozen=True)
class CompatibilityPolicy:
    application_major: int = 19
    application_minor: int = 0
    application_patch: int = 1
    document_major: int = 10
    document_build: str = "26200"

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
        if document.major_version != self.document_major or document.build != self.document_build:
            raise ValueError(
                f"Unsupported ProPresenter document version {document.major_version}/{document.build}"
            )

