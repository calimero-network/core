from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_move_body import AppMoveBody
from ...models.app_move_response_200 import AppMoveResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: AppMoveBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/move".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppMoveResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppMoveResponse200.from_dict(response.json())

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
) -> Response[Union[AppMoveResponse200, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
    body: AppMoveBody,
) -> Response[Union[AppMoveResponse200, Error]]:
    """Move an app to another organization

     Its data where it is; its files, runs and quotas go to the target, `org` or app-team access becomes
    the target's app team of its package. `bringMembers` adds the source's members who reach it, capped
    at `member`. `from` is the organization it left, `null` for an app of the caller's own; `added` the
    accounts brought; `revokedInvites` how many invites to its old team were withdrawn.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppMoveBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppMoveResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    *,
    client: AuthenticatedClient,
    body: AppMoveBody,
) -> Optional[Union[AppMoveResponse200, Error]]:
    """Move an app to another organization

     Its data where it is; its files, runs and quotas go to the target, `org` or app-team access becomes
    the target's app team of its package. `bringMembers` adds the source's members who reach it, capped
    at `member`. `from` is the organization it left, `null` for an app of the caller's own; `added` the
    accounts brought; `revokedInvites` how many invites to its old team were withdrawn.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppMoveBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppMoveResponse200, Error]
    """

    return sync_detailed(
        app=app,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
    body: AppMoveBody,
) -> Response[Union[AppMoveResponse200, Error]]:
    """Move an app to another organization

     Its data where it is; its files, runs and quotas go to the target, `org` or app-team access becomes
    the target's app team of its package. `bringMembers` adds the source's members who reach it, capped
    at `member`. `from` is the organization it left, `null` for an app of the caller's own; `added` the
    accounts brought; `revokedInvites` how many invites to its old team were withdrawn.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppMoveBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppMoveResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    *,
    client: AuthenticatedClient,
    body: AppMoveBody,
) -> Optional[Union[AppMoveResponse200, Error]]:
    """Move an app to another organization

     Its data where it is; its files, runs and quotas go to the target, `org` or app-team access becomes
    the target's app team of its package. `bringMembers` adds the source's members who reach it, capped
    at `member`. `from` is the organization it left, `null` for an app of the caller's own; `added` the
    accounts brought; `revokedInvites` how many invites to its old team were withdrawn.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppMoveBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppMoveResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
