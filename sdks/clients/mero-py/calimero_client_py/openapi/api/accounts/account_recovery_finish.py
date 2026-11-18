from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.account_recovery_finish_body import AccountRecoveryFinishBody
from ...models.error import Error
from ...models.session import Session
from typing import cast


def _get_kwargs(
    *,
    body: AccountRecoveryFinishBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/auth/recovery/finish",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, Session]]:
    if response.status_code == 200:
        response_200 = Session.from_dict(response.json())

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
) -> Response[Union[Error, Session]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: AccountRecoveryFinishBody,
) -> Response[Union[Error, Session]]:
    """Finish an account recovery

     Approved by the new passkey once `readyAt` passed (`412` first): adds it and ends every session of
    the account.

    **Who:** anyone. **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with
    `confirm`, a passkey assertion over `details.challenge`.

    Args:
        body (AccountRecoveryFinishBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, Session]]
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
    body: AccountRecoveryFinishBody,
) -> Optional[Union[Error, Session]]:
    """Finish an account recovery

     Approved by the new passkey once `readyAt` passed (`412` first): adds it and ends every session of
    the account.

    **Who:** anyone. **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with
    `confirm`, a passkey assertion over `details.challenge`.

    Args:
        body (AccountRecoveryFinishBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, Session]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: AccountRecoveryFinishBody,
) -> Response[Union[Error, Session]]:
    """Finish an account recovery

     Approved by the new passkey once `readyAt` passed (`412` first): adds it and ends every session of
    the account.

    **Who:** anyone. **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with
    `confirm`, a passkey assertion over `details.challenge`.

    Args:
        body (AccountRecoveryFinishBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, Session]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: AccountRecoveryFinishBody,
) -> Optional[Union[Error, Session]]:
    """Finish an account recovery

     Approved by the new passkey once `readyAt` passed (`412` first): adds it and ends every session of
    the account.

    **Who:** anyone. **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with
    `confirm`, a passkey assertion over `details.challenge`.

    Args:
        body (AccountRecoveryFinishBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, Session]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
