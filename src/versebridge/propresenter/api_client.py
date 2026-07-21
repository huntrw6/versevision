from __future__ import annotations

from urllib.parse import quote

import httpx
from pydantic import ValidationError

from versebridge.errors import ProPresenterConnectionError, UnsupportedProPresenterError
from .models import (
    Library,
    LibraryContents,
    PlaylistItem,
    PresentationMetadata,
    PresentationResponse,
    PresentationSummary,
    ProPresenterVersion,
)


class ProPresenterApiClient:
    def __init__(self, host: str = "127.0.0.1", port: int = 50001, timeout_seconds: float = 5.0):
        self.base_url = f"http://{host}:{port}"
        self._client = httpx.AsyncClient(base_url=self.base_url, timeout=timeout_seconds)

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_):
        await self.aclose()

    async def aclose(self) -> None:
        await self._client.aclose()

    async def _get(self, route: str):
        try:
            response = await self._client.get(route)
            response.raise_for_status()
            return response.json()
        except httpx.ConnectError as exc:
            raise ProPresenterConnectionError(
                f"Could not connect to ProPresenter at {self.base_url}. Confirm that ProPresenter is running and its Network API is enabled."
            ) from exc
        except httpx.TimeoutException as exc:
            raise ProPresenterConnectionError(f"ProPresenter API timed out at {self.base_url}") from exc
        except httpx.HTTPStatusError as exc:
            raise ProPresenterConnectionError(
                f"ProPresenter API returned HTTP {exc.response.status_code} for {route}"
            ) from exc
        except (ValueError, ValidationError) as exc:
            raise ProPresenterConnectionError(f"Malformed ProPresenter response for {route}: {exc}") from exc

    async def _put(self, route: str, payload):
        try:
            response = await self._client.put(route, json=payload)
            response.raise_for_status()
            return response.json() if response.content else None
        except httpx.ConnectError as exc:
            raise ProPresenterConnectionError(f"Could not connect to ProPresenter at {self.base_url}") from exc
        except httpx.TimeoutException as exc:
            raise ProPresenterConnectionError(f"ProPresenter API timed out at {self.base_url}") from exc
        except httpx.HTTPStatusError as exc:
            raise ProPresenterConnectionError(
                f"ProPresenter API returned HTTP {exc.response.status_code} for {route}"
            ) from exc
        except ValueError as exc:
            raise ProPresenterConnectionError(f"Malformed ProPresenter response for {route}: {exc}") from exc

    async def version(self) -> ProPresenterVersion:
        try:
            version = ProPresenterVersion.model_validate(await self._get("/version"))
        except ValidationError as exc:
            raise ProPresenterConnectionError(f"Malformed ProPresenter version response: {exc}") from exc
        if version.api_version.casefold() != "v1":
            raise UnsupportedProPresenterError(
                f"Unsupported ProPresenter API version: {version.api_version}. No files were changed."
            )
        return version

    async def libraries(self) -> list[Library]:
        try:
            return [Library.model_validate(item) for item in await self._get("/v1/libraries")]
        except ValidationError as exc:
            raise ProPresenterConnectionError(f"Malformed library response: {exc}") from exc

    async def library_presentations(self, library: Library) -> list[PresentationSummary]:
        try:
            contents = LibraryContents.model_validate(await self._get(f"/v1/library/{quote(library.uuid)}"))
        except ValidationError as exc:
            raise ProPresenterConnectionError(f"Malformed presentation listing for {library.name}: {exc}") from exc
        return [
            item.model_copy(update={"library_uuid": library.uuid, "library_name": library.name})
            for item in contents.items
        ]

    async def presentations(self) -> list[PresentationSummary]:
        found: list[PresentationSummary] = []
        for library in await self.libraries():
            found.extend(await self.library_presentations(library))
        return found

    async def presentation(self, uuid: str) -> PresentationMetadata:
        try:
            return PresentationResponse.model_validate(
                await self._get(f"/v1/presentation/{quote(uuid)}")
            ).presentation
        except ValidationError as exc:
            raise ProPresenterConnectionError(f"Malformed presentation response for {uuid}: {exc}") from exc

    async def playlists(self) -> list[PlaylistItem]:
        try:
            return [PlaylistItem.model_validate(item) for item in await self._get("/v1/playlists")]
        except ValidationError as exc:
            raise ProPresenterConnectionError(f"Malformed playlist response: {exc}") from exc

    async def playlist(self, uuid: str) -> PlaylistItem:
        try:
            return PlaylistItem.model_validate(await self._get(f"/v1/playlist/{quote(uuid)}"))
        except ValidationError as exc:
            raise ProPresenterConnectionError(f"Malformed playlist response for {uuid}: {exc}") from exc

    async def replace_playlist(self, uuid: str, playlist: PlaylistItem) -> PlaylistItem:
        """Replace a playlist with a complete API-derived model, then verify it."""
        await self._put(f"/v1/playlist/{quote(uuid)}", playlist.model_dump(mode="json", exclude_none=False))
        return await self.playlist(uuid)
