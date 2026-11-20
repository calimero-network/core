from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_update_response_200 import AppUpdateResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/update".format(
            app=app,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppUpdateResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppUpdateResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

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
) -> Response[Union[AppUpdateResponse200, Error]]:
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
) -> Response[Union[AppUpdateResponse200, Error]]:
    """Whether an update is available for an app

     For whoever may read the app (`GET /apps/{app}`'s rule; a session bound to its package's frontend
    included), so its frontend can offer the update and open the consent page. `available` carries the
    update classifier's verdict (`calimero_bundle::update::classify`; absent for an instance on a
    development bundle), over the whole unit when the instance has members, as `upgrade-unit` would move
    them, else the app alone: `class` `red` when the signer changes, egress grows, a state version
    changes, a member with egress joins or leaves, or a release newly calls another app as the person
    (`reasons` lists each), else `green`; `egress` before and after with the destinations `added` and
    `removed`; `sealedBefore` and `sealedAfter` against the cluster's sealed endpoints; `newMethods` and
    `removedMethods` (a member's as `<path>.<method>`). `available`: for an instance from a deployment,
    the release the deployment offers for its package when it is not the one running (`source:
    deployment`); for any other, the newest release of its package the caller sees, newer than the
    running one, that the upgrade would take (`source: release`, as a report's `latestReachable`); else
    `null`. `mayApprove`: whether the caller may approve the upgrade. `approvers`: `owner-and-admins`
    (the owner and admins of the app's organization), `operator` where the cluster gives it app
    upgrades, or `managers` on a cluster still below 1.0.0-rc.32 (the rule before: the owner or
    `MANAGE_APPLICATION` holders, or the publisher's).

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppUpdateResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AppUpdateResponse200, Error]]:
    """Whether an update is available for an app

     For whoever may read the app (`GET /apps/{app}`'s rule; a session bound to its package's frontend
    included), so its frontend can offer the update and open the consent page. `available` carries the
    update classifier's verdict (`calimero_bundle::update::classify`; absent for an instance on a
    development bundle), over the whole unit when the instance has members, as `upgrade-unit` would move
    them, else the app alone: `class` `red` when the signer changes, egress grows, a state version
    changes, a member with egress joins or leaves, or a release newly calls another app as the person
    (`reasons` lists each), else `green`; `egress` before and after with the destinations `added` and
    `removed`; `sealedBefore` and `sealedAfter` against the cluster's sealed endpoints; `newMethods` and
    `removedMethods` (a member's as `<path>.<method>`). `available`: for an instance from a deployment,
    the release the deployment offers for its package when it is not the one running (`source:
    deployment`); for any other, the newest release of its package the caller sees, newer than the
    running one, that the upgrade would take (`source: release`, as a report's `latestReachable`); else
    `null`. `mayApprove`: whether the caller may approve the upgrade. `approvers`: `owner-and-admins`
    (the owner and admins of the app's organization), `operator` where the cluster gives it app
    upgrades, or `managers` on a cluster still below 1.0.0-rc.32 (the rule before: the owner or
    `MANAGE_APPLICATION` holders, or the publisher's).

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppUpdateResponse200, Error]
    """

    return sync_detailed(
        app=app,
        client=client,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[AppUpdateResponse200, Error]]:
    """Whether an update is available for an app

     For whoever may read the app (`GET /apps/{app}`'s rule; a session bound to its package's frontend
    included), so its frontend can offer the update and open the consent page. `available` carries the
    update classifier's verdict (`calimero_bundle::update::classify`; absent for an instance on a
    development bundle), over the whole unit when the instance has members, as `upgrade-unit` would move
    them, else the app alone: `class` `red` when the signer changes, egress grows, a state version
    changes, a member with egress joins or leaves, or a release newly calls another app as the person
    (`reasons` lists each), else `green`; `egress` before and after with the destinations `added` and
    `removed`; `sealedBefore` and `sealedAfter` against the cluster's sealed endpoints; `newMethods` and
    `removedMethods` (a member's as `<path>.<method>`). `available`: for an instance from a deployment,
    the release the deployment offers for its package when it is not the one running (`source:
    deployment`); for any other, the newest release of its package the caller sees, newer than the
    running one, that the upgrade would take (`source: release`, as a report's `latestReachable`); else
    `null`. `mayApprove`: whether the caller may approve the upgrade. `approvers`: `owner-and-admins`
    (the owner and admins of the app's organization), `operator` where the cluster gives it app
    upgrades, or `managers` on a cluster still below 1.0.0-rc.32 (the rule before: the owner or
    `MANAGE_APPLICATION` holders, or the publisher's).

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppUpdateResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AppUpdateResponse200, Error]]:
    """Whether an update is available for an app

     For whoever may read the app (`GET /apps/{app}`'s rule; a session bound to its package's frontend
    included), so its frontend can offer the update and open the consent page. `available` carries the
    update classifier's verdict (`calimero_bundle::update::classify`; absent for an instance on a
    development bundle), over the whole unit when the instance has members, as `upgrade-unit` would move
    them, else the app alone: `class` `red` when the signer changes, egress grows, a state version
    changes, a member with egress joins or leaves, or a release newly calls another app as the person
    (`reasons` lists each), else `green`; `egress` before and after with the destinations `added` and
    `removed`; `sealedBefore` and `sealedAfter` against the cluster's sealed endpoints; `newMethods` and
    `removedMethods` (a member's as `<path>.<method>`). `available`: for an instance from a deployment,
    the release the deployment offers for its package when it is not the one running (`source:
    deployment`); for any other, the newest release of its package the caller sees, newer than the
    running one, that the upgrade would take (`source: release`, as a report's `latestReachable`); else
    `null`. `mayApprove`: whether the caller may approve the upgrade. `approvers`: `owner-and-admins`
    (the owner and admins of the app's organization), `operator` where the cluster gives it app
    upgrades, or `managers` on a cluster still below 1.0.0-rc.32 (the rule before: the owner or
    `MANAGE_APPLICATION` holders, or the publisher's).

    **Who:** whoever the app's grants admit (`anyone`: any signed-in account).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppUpdateResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
        )
    ).parsed
