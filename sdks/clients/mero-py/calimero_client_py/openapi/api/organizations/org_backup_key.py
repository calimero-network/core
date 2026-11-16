from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.backup_key_answer import BackupKeyAnswer
from ...models.error import Error
from typing import cast


def _get_kwargs(
    org: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/orgs/{org}/backup-key".format(
            org=org,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[BackupKeyAnswer, Error]]:
    if response.status_code == 200:
        response_200 = BackupKeyAnswer.from_dict(response.json())

        return response_200

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

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[BackupKeyAnswer, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    org: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[BackupKeyAnswer, Error]]:
    """The organization's backup key

     The X25519 public key its apps' recovery-disk backups are sealed to (`key`, `null` while none is
    set), and how many apps that covers. Any member; `404` outside the organization.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[BackupKeyAnswer, Error]]
    """

    kwargs = _get_kwargs(
        org=org,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    org: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[BackupKeyAnswer, Error]]:
    """The organization's backup key

     The X25519 public key its apps' recovery-disk backups are sealed to (`key`, `null` while none is
    set), and how many apps that covers. Any member; `404` outside the organization.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[BackupKeyAnswer, Error]
    """

    return sync_detailed(
        org=org,
        client=client,
    ).parsed


async def asyncio_detailed(
    org: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[BackupKeyAnswer, Error]]:
    """The organization's backup key

     The X25519 public key its apps' recovery-disk backups are sealed to (`key`, `null` while none is
    set), and how many apps that covers. Any member; `404` outside the organization.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[BackupKeyAnswer, Error]]
    """

    kwargs = _get_kwargs(
        org=org,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    org: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[BackupKeyAnswer, Error]]:
    """The organization's backup key

     The X25519 public key its apps' recovery-disk backups are sealed to (`key`, `null` while none is
    set), and how many apps that covers. Any member; `404` outside the organization.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[BackupKeyAnswer, Error]
    """

    return (
        await asyncio_detailed(
            org=org,
            client=client,
        )
    ).parsed
