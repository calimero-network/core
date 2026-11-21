from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.org_updates_response_200 import OrgUpdatesResponse200
from ...types import UNSET, Unset
from typing import cast
from typing import Union


def _get_kwargs(
    org: str,
    *,
    after: Union[Unset, str] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["after"] = after

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/orgs/{org}/updates".format(
            org=org,
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OrgUpdatesResponse200]]:
    if response.status_code == 200:
        response_200 = OrgUpdatesResponse200.from_dict(response.json())

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
) -> Response[Union[Error, OrgUpdatesResponse200]]:
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
    after: Union[Unset, str] = UNSET,
) -> Response[Union[Error, OrgUpdatesResponse200]]:
    """Updates waiting across an organization's apps

     For its owner and admins (`403` to other members, `404` to anyone else): its apps, by id, at most 50
    read per page, that have an update available or a first approval waiting, each as `GET
    /apps/{app}/update` answers it with the app's `name`; `policy` is the organization's; `next` is the
    `after` of the next page, `null` at the end.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):
        after (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OrgUpdatesResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        after=after,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    org: str,
    *,
    client: AuthenticatedClient,
    after: Union[Unset, str] = UNSET,
) -> Optional[Union[Error, OrgUpdatesResponse200]]:
    """Updates waiting across an organization's apps

     For its owner and admins (`403` to other members, `404` to anyone else): its apps, by id, at most 50
    read per page, that have an update available or a first approval waiting, each as `GET
    /apps/{app}/update` answers it with the app's `name`; `policy` is the organization's; `next` is the
    `after` of the next page, `null` at the end.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):
        after (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OrgUpdatesResponse200]
    """

    return sync_detailed(
        org=org,
        client=client,
        after=after,
    ).parsed


async def asyncio_detailed(
    org: str,
    *,
    client: AuthenticatedClient,
    after: Union[Unset, str] = UNSET,
) -> Response[Union[Error, OrgUpdatesResponse200]]:
    """Updates waiting across an organization's apps

     For its owner and admins (`403` to other members, `404` to anyone else): its apps, by id, at most 50
    read per page, that have an update available or a first approval waiting, each as `GET
    /apps/{app}/update` answers it with the app's `name`; `policy` is the organization's; `next` is the
    `after` of the next page, `null` at the end.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):
        after (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OrgUpdatesResponse200]]
    """

    kwargs = _get_kwargs(
        org=org,
        after=after,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    org: str,
    *,
    client: AuthenticatedClient,
    after: Union[Unset, str] = UNSET,
) -> Optional[Union[Error, OrgUpdatesResponse200]]:
    """Updates waiting across an organization's apps

     For its owner and admins (`403` to other members, `404` to anyone else): its apps, by id, at most 50
    read per page, that have an update available or a first approval waiting, each as `GET
    /apps/{app}/update` answers it with the app's `name`; `policy` is the organization's; `next` is the
    `after` of the next page, `null` at the end.

    **Who:** an account holder (a passkey session not bound to an app's frontend).

    Args:
        org (str):
        after (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OrgUpdatesResponse200]
    """

    return (
        await asyncio_detailed(
            org=org,
            client=client,
            after=after,
        )
    ).parsed
