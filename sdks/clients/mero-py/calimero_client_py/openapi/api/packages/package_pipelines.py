from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.release_pipelines import ReleasePipelines
from typing import cast


def _get_kwargs(
    package: str,
    version: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/packages/{package}/{version}/pipelines".format(
            package=package,
            version=version,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, ReleasePipelines]]:
    if response.status_code == 200:
        response_200 = ReleasePipelines.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 404:
        response_404 = Error.from_dict(response.json())

        return response_404

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, ReleasePipelines]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    package: str,
    version: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, ReleasePipelines]]:
    """A release's pipelines

     The pipelines the release registers, by name: `hash`, `worstCaseFuel` (a decimal string), the
    `modules` they pin and the canonical `pipeline`. `404` for a release the caller may not see.

    **Who:** signed in (a session or an API key).

    Args:
        package (str):
        version (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, ReleasePipelines]]
    """

    kwargs = _get_kwargs(
        package=package,
        version=version,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    package: str,
    version: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, ReleasePipelines]]:
    """A release's pipelines

     The pipelines the release registers, by name: `hash`, `worstCaseFuel` (a decimal string), the
    `modules` they pin and the canonical `pipeline`. `404` for a release the caller may not see.

    **Who:** signed in (a session or an API key).

    Args:
        package (str):
        version (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, ReleasePipelines]
    """

    return sync_detailed(
        package=package,
        version=version,
        client=client,
    ).parsed


async def asyncio_detailed(
    package: str,
    version: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, ReleasePipelines]]:
    """A release's pipelines

     The pipelines the release registers, by name: `hash`, `worstCaseFuel` (a decimal string), the
    `modules` they pin and the canonical `pipeline`. `404` for a release the caller may not see.

    **Who:** signed in (a session or an API key).

    Args:
        package (str):
        version (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, ReleasePipelines]]
    """

    kwargs = _get_kwargs(
        package=package,
        version=version,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    package: str,
    version: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, ReleasePipelines]]:
    """A release's pipelines

     The pipelines the release registers, by name: `hash`, `worstCaseFuel` (a decimal string), the
    `modules` they pin and the canonical `pipeline`. `404` for a release the caller may not see.

    **Who:** signed in (a session or an API key).

    Args:
        package (str):
        version (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, ReleasePipelines]
    """

    return (
        await asyncio_detailed(
            package=package,
            version=version,
            client=client,
        )
    ).parsed
