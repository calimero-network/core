from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.exports_export_body import ExportsExportBody
from ...models.exports_export_response_200 import ExportsExportResponse200
from typing import cast


def _get_kwargs(
    app: str,
    id: str,
    *,
    body: ExportsExportBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/files/{id}/export".format(
            app=app,
            id=id,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, ExportsExportResponse200]]:
    if response.status_code == 200:
        response_200 = ExportsExportResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = Error.from_dict(response.json())

        return response_400

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if response.status_code == 404:
        response_404 = Error.from_dict(response.json())

        return response_404

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 429:
        response_429 = Error.from_dict(response.json())

        return response_429

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, ExportsExportResponse200]]:
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
    body: ExportsExportBody,
) -> Response[Union[Error, ExportsExportResponse200]]:
    """Export a sealed file's key

     The owner and the organization's members. `recipient` is an X25519 public key (64 hex characters);
    the file's data key comes back wrapped to it, with the file's `segments` (each one's hash, hex) and
    `ephemeral` public key. The node never answers plaintext or a key in the clear. 30 a minute per
    account.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        id (str):
        body (ExportsExportBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, ExportsExportResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        id=id,
        body=body,
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
    body: ExportsExportBody,
) -> Optional[Union[Error, ExportsExportResponse200]]:
    """Export a sealed file's key

     The owner and the organization's members. `recipient` is an X25519 public key (64 hex characters);
    the file's data key comes back wrapped to it, with the file's `segments` (each one's hash, hex) and
    `ephemeral` public key. The node never answers plaintext or a key in the clear. 30 a minute per
    account.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        id (str):
        body (ExportsExportBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, ExportsExportResponse200]
    """

    return sync_detailed(
        app=app,
        id=id,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    app: str,
    id: str,
    *,
    client: AuthenticatedClient,
    body: ExportsExportBody,
) -> Response[Union[Error, ExportsExportResponse200]]:
    """Export a sealed file's key

     The owner and the organization's members. `recipient` is an X25519 public key (64 hex characters);
    the file's data key comes back wrapped to it, with the file's `segments` (each one's hash, hex) and
    `ephemeral` public key. The node never answers plaintext or a key in the clear. 30 a minute per
    account.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        id (str):
        body (ExportsExportBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, ExportsExportResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        id=id,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    id: str,
    *,
    client: AuthenticatedClient,
    body: ExportsExportBody,
) -> Optional[Union[Error, ExportsExportResponse200]]:
    """Export a sealed file's key

     The owner and the organization's members. `recipient` is an X25519 public key (64 hex characters);
    the file's data key comes back wrapped to it, with the file's `segments` (each one's hash, hex) and
    `ephemeral` public key. The node never answers plaintext or a key in the clear. 30 a minute per
    account.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        id (str):
        body (ExportsExportBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, ExportsExportResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            id=id,
            client=client,
            body=body,
        )
    ).parsed
