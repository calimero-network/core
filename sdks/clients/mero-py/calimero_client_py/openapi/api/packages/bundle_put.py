from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.bundle_put_response_200 import BundlePutResponse200
from ...models.error import Error
from ...types import File, FileTypes
from io import BytesIO
from typing import cast


def _get_kwargs(
    *,
    body: File,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/bundles",
    }

    _kwargs["content"] = body.payload

    headers["Content-Type"] = "application/wasm"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[BundlePutResponse200, Error]]:
    if response.status_code == 200:
        response_200 = BundlePutResponse200.from_dict(response.json())

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

    if response.status_code == 413:
        response_413 = Error.from_dict(response.json())

        return response_413

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
) -> Response[Union[BundlePutResponse200, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    body: File,
) -> Response[Union[BundlePutResponse200, Error]]:
    """Upload a raw component

     A `calimero:app` component, 16 MiB at most; only on a node started with `--allow-unsigned-bundles`
    or `--dev`. A WASM core module (SDK 0.12) is refused `400 UNSUPPORTED_BUNDLE`.

    **Who:** an installer (an account holder, or an unscoped API key that is not read-only).

    Args:
        body (File):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[BundlePutResponse200, Error]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    body: File,
) -> Optional[Union[BundlePutResponse200, Error]]:
    """Upload a raw component

     A `calimero:app` component, 16 MiB at most; only on a node started with `--allow-unsigned-bundles`
    or `--dev`. A WASM core module (SDK 0.12) is refused `400 UNSUPPORTED_BUNDLE`.

    **Who:** an installer (an account holder, or an unscoped API key that is not read-only).

    Args:
        body (File):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[BundlePutResponse200, Error]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    body: File,
) -> Response[Union[BundlePutResponse200, Error]]:
    """Upload a raw component

     A `calimero:app` component, 16 MiB at most; only on a node started with `--allow-unsigned-bundles`
    or `--dev`. A WASM core module (SDK 0.12) is refused `400 UNSUPPORTED_BUNDLE`.

    **Who:** an installer (an account holder, or an unscoped API key that is not read-only).

    Args:
        body (File):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[BundlePutResponse200, Error]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    body: File,
) -> Optional[Union[BundlePutResponse200, Error]]:
    """Upload a raw component

     A `calimero:app` component, 16 MiB at most; only on a node started with `--allow-unsigned-bundles`
    or `--dev`. A WASM core module (SDK 0.12) is refused `400 UNSUPPORTED_BUNDLE`.

    **Who:** an installer (an account holder, or an unscoped API key that is not read-only).

    Args:
        body (File):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[BundlePutResponse200, Error]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
