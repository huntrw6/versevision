import asyncio

import httpx
import pytest
import respx

from versevision.errors import ConfigurationError, ProPresenterConnectionError, UnsupportedProPresenterError
from versevision.propresenter.api_client import ProPresenterApiClient
from versevision.propresenter.models import PresentationSummary
from versevision.propresenter.selection import select_unique_presentation


@respx.mock
def test_api_discovers_presentations_with_library_context() -> None:
    respx.get("http://127.0.0.1:50001/v1/libraries").mock(return_value=httpx.Response(200, json=[
        {"uuid": "library-1", "name": "Sunday", "index": 0}
    ]))
    respx.get("http://127.0.0.1:50001/v1/library/library-1").mock(return_value=httpx.Response(200, json={
        "update_type": "full", "items": [{"uuid": "presentation-1", "name": "Message", "index": 0}]
    }))

    async def run():
        async with ProPresenterApiClient() as client:
            return await client.presentations()

    assert asyncio.run(run()) == [PresentationSummary(
        uuid="presentation-1", name="Message", index=0,
        library_uuid="library-1", library_name="Sunday",
    )]


@respx.mock
def test_api_rejects_malformed_response() -> None:
    respx.get("http://127.0.0.1:50001/version").mock(return_value=httpx.Response(200, json={"name": "ProPresenter"}))

    async def run():
        async with ProPresenterApiClient() as client:
            await client.version()

    with pytest.raises(ProPresenterConnectionError, match="Malformed ProPresenter version"):
        asyncio.run(run())


@respx.mock
def test_api_rejects_unsupported_api_version_without_writing() -> None:
    respx.get("http://127.0.0.1:50001/version").mock(return_value=httpx.Response(200, json={
        "name": "host", "platform": "win", "os_version": "1",
        "host_description": "ProPresenter future", "api_version": "v2",
    }))

    async def run():
        async with ProPresenterApiClient() as client:
            await client.version()

    with pytest.raises(UnsupportedProPresenterError, match="No files were changed"):
        asyncio.run(run())


def test_target_name_must_be_unique() -> None:
    choices = [
        PresentationSummary(uuid="one", name="Message", index=0, library_name="Sunday"),
        PresentationSummary(uuid="two", name="Message", index=0, library_name="Archive"),
    ]
    with pytest.raises(ConfigurationError, match="ambiguous"):
        select_unique_presentation(choices, name="Message")
    assert select_unique_presentation(choices, name="Message", library="Archive").uuid == "two"
