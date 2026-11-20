from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_approve_upgrade_body import AppApproveUpgradeBody
from ...models.app_approve_upgrade_response_200 import AppApproveUpgradeResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: AppApproveUpgradeBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/upgrade-approvals".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppApproveUpgradeResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppApproveUpgradeResponse200.from_dict(response.json())

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

    if response.status_code == 404:
        response_404 = Error.from_dict(response.json())

        return response_404

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
) -> Response[Union[AppApproveUpgradeResponse200, Error]]:
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
    body: AppApproveUpgradeBody,
) -> Response[Union[AppApproveUpgradeResponse200, Error]]:
    """Approve a red update first, for a second approver to apply

     Where the app's policy asks for a second approver (`409 ONE_APPROVAL` otherwise, and for a green
    update), a red update needs two distinct owners or admins of its organization: this logs the first
    approval, its own consent over `<app> / __approve_upgrade / { package, version, targets, class }`
    (the class the classifier gives now, over the unit with `unit`). Another owner or admin then applies
    it with `POST /apps/{app}/upgrade` (or `/upgrade-unit` for a unit's approval), which until then
    answers `409 SECOND_APPROVAL_REQUIRED`. One waits per app, for a week at most; a newer one replaces
    it, and another approver's of the same update is `409 APPROVAL_PENDING` (apply it instead). `409
    FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppApproveUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppApproveUpgradeResponse200, Error]]
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
    body: AppApproveUpgradeBody,
) -> Optional[Union[AppApproveUpgradeResponse200, Error]]:
    """Approve a red update first, for a second approver to apply

     Where the app's policy asks for a second approver (`409 ONE_APPROVAL` otherwise, and for a green
    update), a red update needs two distinct owners or admins of its organization: this logs the first
    approval, its own consent over `<app> / __approve_upgrade / { package, version, targets, class }`
    (the class the classifier gives now, over the unit with `unit`). Another owner or admin then applies
    it with `POST /apps/{app}/upgrade` (or `/upgrade-unit` for a unit's approval), which until then
    answers `409 SECOND_APPROVAL_REQUIRED`. One waits per app, for a week at most; a newer one replaces
    it, and another approver's of the same update is `409 APPROVAL_PENDING` (apply it instead). `409
    FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppApproveUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppApproveUpgradeResponse200, Error]
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
    body: AppApproveUpgradeBody,
) -> Response[Union[AppApproveUpgradeResponse200, Error]]:
    """Approve a red update first, for a second approver to apply

     Where the app's policy asks for a second approver (`409 ONE_APPROVAL` otherwise, and for a green
    update), a red update needs two distinct owners or admins of its organization: this logs the first
    approval, its own consent over `<app> / __approve_upgrade / { package, version, targets, class }`
    (the class the classifier gives now, over the unit with `unit`). Another owner or admin then applies
    it with `POST /apps/{app}/upgrade` (or `/upgrade-unit` for a unit's approval), which until then
    answers `409 SECOND_APPROVAL_REQUIRED`. One waits per app, for a week at most; a newer one replaces
    it, and another approver's of the same update is `409 APPROVAL_PENDING` (apply it instead). `409
    FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppApproveUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppApproveUpgradeResponse200, Error]]
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
    body: AppApproveUpgradeBody,
) -> Optional[Union[AppApproveUpgradeResponse200, Error]]:
    """Approve a red update first, for a second approver to apply

     Where the app's policy asks for a second approver (`409 ONE_APPROVAL` otherwise, and for a green
    update), a red update needs two distinct owners or admins of its organization: this logs the first
    approval, its own consent over `<app> / __approve_upgrade / { package, version, targets, class }`
    (the class the classifier gives now, over the unit with `unit`). Another owner or admin then applies
    it with `POST /apps/{app}/upgrade` (or `/upgrade-unit` for a unit's approval), which until then
    answers `409 SECOND_APPROVAL_REQUIRED`. One waits per app, for a week at most; a newer one replaces
    it, and another approver's of the same update is `409 APPROVAL_PENDING` (apply it instead). `409
    FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppApproveUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppApproveUpgradeResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
