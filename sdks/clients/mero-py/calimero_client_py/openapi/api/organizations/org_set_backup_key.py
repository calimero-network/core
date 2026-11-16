from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.backup_key_answer import BackupKeyAnswer
from ...models.error import Error
from ...models.org_set_backup_key_body import OrgSetBackupKeyBody
from typing import cast


def _get_kwargs(
    org: str,
    *,
    body: OrgSetBackupKeyBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/orgs/{org}/backup-key".format(
            org=org,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[BackupKeyAnswer, Error]]:
    if response.status_code == 200:
        response_200 = BackupKeyAnswer.from_dict(response.json())

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

    if response.status_code == 412:
        response_412 = Error.from_dict(response.json())

        return response_412

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
    body: OrgSetBackupKeyBody,
) -> Response[Union[BackupKeyAnswer, Error]]:
    """Set the organization's backup key

     `publicKey`: 64 lowercase hex, an X25519 public key whose private half its holder keeps (never a
    node). Those who upgrade all the organization's apps (org-wide `MANAGE_APPLICATION`), where owners
    upgrade apps; replaces the key before. `412` first.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        org (str):
        body (OrgSetBackupKeyBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[BackupKeyAnswer, Error]]
    """

    kwargs = _get_kwargs(
        org=org,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    org: str,
    *,
    client: AuthenticatedClient,
    body: OrgSetBackupKeyBody,
) -> Optional[Union[BackupKeyAnswer, Error]]:
    """Set the organization's backup key

     `publicKey`: 64 lowercase hex, an X25519 public key whose private half its holder keeps (never a
    node). Those who upgrade all the organization's apps (org-wide `MANAGE_APPLICATION`), where owners
    upgrade apps; replaces the key before. `412` first.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        org (str):
        body (OrgSetBackupKeyBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[BackupKeyAnswer, Error]
    """

    return sync_detailed(
        org=org,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    org: str,
    *,
    client: AuthenticatedClient,
    body: OrgSetBackupKeyBody,
) -> Response[Union[BackupKeyAnswer, Error]]:
    """Set the organization's backup key

     `publicKey`: 64 lowercase hex, an X25519 public key whose private half its holder keeps (never a
    node). Those who upgrade all the organization's apps (org-wide `MANAGE_APPLICATION`), where owners
    upgrade apps; replaces the key before. `412` first.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        org (str):
        body (OrgSetBackupKeyBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[BackupKeyAnswer, Error]]
    """

    kwargs = _get_kwargs(
        org=org,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    org: str,
    *,
    client: AuthenticatedClient,
    body: OrgSetBackupKeyBody,
) -> Optional[Union[BackupKeyAnswer, Error]]:
    """Set the organization's backup key

     `publicKey`: 64 lowercase hex, an X25519 public key whose private half its holder keeps (never a
    node). Those who upgrade all the organization's apps (org-wide `MANAGE_APPLICATION`), where owners
    upgrade apps; replaces the key before. `412` first.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        org (str):
        body (OrgSetBackupKeyBody):

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
            body=body,
        )
    ).parsed
