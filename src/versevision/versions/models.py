from pydantic import BaseModel


class BibleVersion(BaseModel):
    abbreviation: str
    name: str
    language: str = "eng"
    provider: str | None = None
    provider_version_id: str | None = None
    copyright: str | None = None
    attribution: str | None = None

