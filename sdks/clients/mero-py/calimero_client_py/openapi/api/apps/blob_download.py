from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...types import File, FileTypes
from ...types import UNSET, Unset
from io import BytesIO
from typing import cast
from typing import Union


def _get_kwargs(
    app: str,
    id: str,
    *,
    grant: Union[Unset, str] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["grant"] = grant

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/blobs/{id}".format(
            app=app,
            id=id,
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, File]]:
    if response.status_code == 200:
        response_200 = File(payload=BytesIO(response.content))

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

    if response.status_code == 503:
        response_503 = Error.from_dict(response.json())

        return response_503

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, File]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    app: str,
    id: str,
    *,
    client: AuthenticatedClient,
    grant: Union[Unset, str] = UNSET,
) -> Response[Union[Error, File]]:
    """Download a blob

     `{id}` is base58 or hex; `grant` is a link the app made (`BlobRef::grant`).

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        id (str):
        grant (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, File]]
    """

    kwargs = _get_kwargs(
        app=app,
        id=id,
        grant=grant,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    id: str,
    *,
    client: AuthenticatedClient,
    grant: Union[Unset, str] = UNSET,
) -> Optional[Union[Error, File]]:
    """Download a blob

     `{id}` is base58 or hex; `grant` is a link the app made (`BlobRef::grant`).

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        id (str):
        grant (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, File]
    """

    return sync_detailed(
        app=app,
        id=id,
        client=client,
        grant=grant,
    ).parsed


async def asyncio_detailed(
    app: str,
    id: str,
    *,
    client: AuthenticatedClient,
    grant: Union[Unset, str] = UNSET,
) -> Response[Union[Error, File]]:
    """Download a blob

     `{id}` is base58 or hex; `grant` is a link the app made (`BlobRef::grant`).

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        id (str):
        grant (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, File]]
    """

    kwargs = _get_kwargs(
        app=app,
        id=id,
        grant=grant,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    id: str,
    *,
    client: AuthenticatedClient,
    grant: Union[Unset, str] = UNSET,
) -> Optional[Union[Error, File]]:
    """Download a blob

     `{id}` is base58 or hex; `grant` is a link the app made (`BlobRef::grant`).

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        id (str):
        grant (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, File]
    """

    return (
        await asyncio_detailed(
            app=app,
            id=id,
            client=client,
            grant=grant,
        )
    ).parsed
