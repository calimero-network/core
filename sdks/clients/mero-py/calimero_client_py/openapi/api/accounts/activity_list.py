from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.activity_list_response_200 import ActivityListResponse200
from ...models.error import Error
from ...types import UNSET, Unset
from typing import cast
from typing import Union


def _get_kwargs(
    *,
    after: Union[Unset, int] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["after"] = after

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/auth/activity",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ActivityListResponse200, Error]]:
    if response.status_code == 200:
        response_200 = ActivityListResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[ActivityListResponse200, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    after: Union[Unset, int] = UNSET,
) -> Response[Union[ActivityListResponse200, Error]]:
    """What the account's passkeys did

     Newest first, the last 100: sign-ins (`app`, `origin`) and approvals (`method`, `subject`); `after`
    keeps the entries after that Unix second.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        after (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ActivityListResponse200, Error]]
    """

    kwargs = _get_kwargs(
        after=after,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    after: Union[Unset, int] = UNSET,
) -> Optional[Union[ActivityListResponse200, Error]]:
    """What the account's passkeys did

     Newest first, the last 100: sign-ins (`app`, `origin`) and approvals (`method`, `subject`); `after`
    keeps the entries after that Unix second.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        after (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ActivityListResponse200, Error]
    """

    return sync_detailed(
        client=client,
        after=after,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    after: Union[Unset, int] = UNSET,
) -> Response[Union[ActivityListResponse200, Error]]:
    """What the account's passkeys did

     Newest first, the last 100: sign-ins (`app`, `origin`) and approvals (`method`, `subject`); `after`
    keeps the entries after that Unix second.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        after (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ActivityListResponse200, Error]]
    """

    kwargs = _get_kwargs(
        after=after,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    after: Union[Unset, int] = UNSET,
) -> Optional[Union[ActivityListResponse200, Error]]:
    """What the account's passkeys did

     Newest first, the last 100: sign-ins (`app`, `origin`) and approvals (`method`, `subject`); `after`
    keeps the entries after that Unix second.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        after (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ActivityListResponse200, Error]
    """

    return (
        await asyncio_detailed(
            client=client,
            after=after,
        )
    ).parsed
