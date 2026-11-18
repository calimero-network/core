from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.ops_backup_response_200 import OpsBackupResponse200
from ...types import UNSET, Unset
from typing import cast
from typing import Union


def _get_kwargs(
    *,
    all_: Union[Unset, bool] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["all"] = all_

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/ops/backup",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OpsBackupResponse200]]:
    if response.status_code == 200:
        response_200 = OpsBackupResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, OpsBackupResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    all_: Union[Unset, bool] = UNSET,
) -> Response[Union[Error, OpsBackupResponse200]]:
    """Snapshot the apps this node leads

     Into `--backup-dir`. With `all=true`, every group this node holds, leader or not: what a node of a
    cluster that lost a majority still has, to found the cluster again from (`merod --bootstrap
    --restore-from`). `written`: how many snapshots it wrote.

    **Who:** the ops token.

    Args:
        all_ (Union[Unset, bool]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsBackupResponse200]]
    """

    kwargs = _get_kwargs(
        all_=all_,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    all_: Union[Unset, bool] = UNSET,
) -> Optional[Union[Error, OpsBackupResponse200]]:
    """Snapshot the apps this node leads

     Into `--backup-dir`. With `all=true`, every group this node holds, leader or not: what a node of a
    cluster that lost a majority still has, to found the cluster again from (`merod --bootstrap
    --restore-from`). `written`: how many snapshots it wrote.

    **Who:** the ops token.

    Args:
        all_ (Union[Unset, bool]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsBackupResponse200]
    """

    return sync_detailed(
        client=client,
        all_=all_,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    all_: Union[Unset, bool] = UNSET,
) -> Response[Union[Error, OpsBackupResponse200]]:
    """Snapshot the apps this node leads

     Into `--backup-dir`. With `all=true`, every group this node holds, leader or not: what a node of a
    cluster that lost a majority still has, to found the cluster again from (`merod --bootstrap
    --restore-from`). `written`: how many snapshots it wrote.

    **Who:** the ops token.

    Args:
        all_ (Union[Unset, bool]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsBackupResponse200]]
    """

    kwargs = _get_kwargs(
        all_=all_,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    all_: Union[Unset, bool] = UNSET,
) -> Optional[Union[Error, OpsBackupResponse200]]:
    """Snapshot the apps this node leads

     Into `--backup-dir`. With `all=true`, every group this node holds, leader or not: what a node of a
    cluster that lost a majority still has, to found the cluster again from (`merod --bootstrap
    --restore-from`). `written`: how many snapshots it wrote.

    **Who:** the ops token.

    Args:
        all_ (Union[Unset, bool]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsBackupResponse200]
    """

    return (
        await asyncio_detailed(
            client=client,
            all_=all_,
        )
    ).parsed
