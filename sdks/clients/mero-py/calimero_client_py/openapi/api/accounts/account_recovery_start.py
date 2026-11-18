from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.account_recovery_start_body import AccountRecoveryStartBody
from ...models.account_recovery_start_response_200 import (
    AccountRecoveryStartResponse200,
)
from ...models.error import Error
from typing import cast


def _get_kwargs(
    *,
    body: AccountRecoveryStartBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/auth/recovery/start",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AccountRecoveryStartResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AccountRecoveryStartResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = Error.from_dict(response.json())

        return response_400

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 409:
        response_409 = Error.from_dict(response.json())

        return response_409

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[AccountRecoveryStartResponse200, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: AccountRecoveryStartBody,
) -> Response[Union[AccountRecoveryStartResponse200, Error]]:
    """Spend the recovery code

     Logs the new passkey as pending; it may finish the recovery once `readyAt` passed (72 h). `401` for
    a wrong or spent code.

    **Who:** anyone.

    Args:
        body (AccountRecoveryStartBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AccountRecoveryStartResponse200, Error]]
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
    client: Union[AuthenticatedClient, Client],
    body: AccountRecoveryStartBody,
) -> Optional[Union[AccountRecoveryStartResponse200, Error]]:
    """Spend the recovery code

     Logs the new passkey as pending; it may finish the recovery once `readyAt` passed (72 h). `401` for
    a wrong or spent code.

    **Who:** anyone.

    Args:
        body (AccountRecoveryStartBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AccountRecoveryStartResponse200, Error]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: AccountRecoveryStartBody,
) -> Response[Union[AccountRecoveryStartResponse200, Error]]:
    """Spend the recovery code

     Logs the new passkey as pending; it may finish the recovery once `readyAt` passed (72 h). `401` for
    a wrong or spent code.

    **Who:** anyone.

    Args:
        body (AccountRecoveryStartBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AccountRecoveryStartResponse200, Error]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: AccountRecoveryStartBody,
) -> Optional[Union[AccountRecoveryStartResponse200, Error]]:
    """Spend the recovery code

     Logs the new passkey as pending; it may finish the recovery once `readyAt` passed (72 h). `401` for
    a wrong or spent code.

    **Who:** anyone.

    Args:
        body (AccountRecoveryStartBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AccountRecoveryStartResponse200, Error]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
