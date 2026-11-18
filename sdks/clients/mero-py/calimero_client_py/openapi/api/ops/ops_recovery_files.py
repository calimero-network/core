from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.ops_recovery_files_response_200 import OpsRecoveryFilesResponse200
from ...types import UNSET, Unset
from typing import cast
from typing import Union


def _get_kwargs(
    *,
    holder: Union[Unset, str] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["holder"] = holder

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/ops/recovery-files",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OpsRecoveryFilesResponse200]]:
    if response.status_code == 200:
        response_200 = OpsRecoveryFilesResponse200.from_dict(response.json())

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

    if response.status_code == 409:
        response_409 = Error.from_dict(response.json())

        return response_409

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, OpsRecoveryFilesResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    holder: Union[Unset, str] = UNSET,
) -> Response[Union[Error, OpsRecoveryFilesResponse200]]:
    r"""The recovery files a node reads

     In `--recover-from`, else `--recovery-dir`, of `holder` (`operator`, the default; `org:<id>`;
    `account:<id>`): each file's name, its header as signed (JSON text) and the node's Ed25519 signature
    over `calimero.recovery-backup.v1\0` and those bytes (hex), for the holder's device to check before
    it opens a key with its own.

    **Who:** the ops token.

    Args:
        holder (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsRecoveryFilesResponse200]]
    """

    kwargs = _get_kwargs(
        holder=holder,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    holder: Union[Unset, str] = UNSET,
) -> Optional[Union[Error, OpsRecoveryFilesResponse200]]:
    r"""The recovery files a node reads

     In `--recover-from`, else `--recovery-dir`, of `holder` (`operator`, the default; `org:<id>`;
    `account:<id>`): each file's name, its header as signed (JSON text) and the node's Ed25519 signature
    over `calimero.recovery-backup.v1\0` and those bytes (hex), for the holder's device to check before
    it opens a key with its own.

    **Who:** the ops token.

    Args:
        holder (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsRecoveryFilesResponse200]
    """

    return sync_detailed(
        client=client,
        holder=holder,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    holder: Union[Unset, str] = UNSET,
) -> Response[Union[Error, OpsRecoveryFilesResponse200]]:
    r"""The recovery files a node reads

     In `--recover-from`, else `--recovery-dir`, of `holder` (`operator`, the default; `org:<id>`;
    `account:<id>`): each file's name, its header as signed (JSON text) and the node's Ed25519 signature
    over `calimero.recovery-backup.v1\0` and those bytes (hex), for the holder's device to check before
    it opens a key with its own.

    **Who:** the ops token.

    Args:
        holder (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsRecoveryFilesResponse200]]
    """

    kwargs = _get_kwargs(
        holder=holder,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    holder: Union[Unset, str] = UNSET,
) -> Optional[Union[Error, OpsRecoveryFilesResponse200]]:
    r"""The recovery files a node reads

     In `--recover-from`, else `--recovery-dir`, of `holder` (`operator`, the default; `org:<id>`;
    `account:<id>`): each file's name, its header as signed (JSON text) and the node's Ed25519 signature
    over `calimero.recovery-backup.v1\0` and those bytes (hex), for the holder's device to check before
    it opens a key with its own.

    **Who:** the ops token.

    Args:
        holder (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsRecoveryFilesResponse200]
    """

    return (
        await asyncio_detailed(
            client=client,
            holder=holder,
        )
    ).parsed
