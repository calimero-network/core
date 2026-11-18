from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_upgrade_body import AppUpgradeBody
from ...models.app_upgrade_response_200 import AppUpgradeResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: AppUpgradeBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/upgrade".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppUpgradeResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppUpgradeResponse200.from_dict(response.json())

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

    if response.status_code == 422:
        response_422 = Error.from_dict(response.json())

        return response_422

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[AppUpgradeResponse200, Error]]:
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
    body: AppUpgradeBody,
) -> Response[Union[AppUpgradeResponse200, Error]]:
    """Upgrade an app instance

     To a release (`version`, of the app's own package unless `package` names another) or a `bundle`,
    code only: the state stays and is read by the new code. Whoever the package's manifest names
    (`upgrades`: the owner, or its publisher's `MANAGE_APPLICATION` holders), with a step-up over `{
    bundle, migrate, manifest }`; an upgrade key signs in place of it. A refused or failed upgrade
    leaves the app on its release and answers `422 UPGRADE_FAILED` with the app log's error in `details`
    (`MigrationsNotSupported` when `migrate` is named: there are no migrations in this version). `error`
    is the app log's error value (`null` when it ran), free-form: the app's own.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppUpgradeResponse200, Error]]
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
    body: AppUpgradeBody,
) -> Optional[Union[AppUpgradeResponse200, Error]]:
    """Upgrade an app instance

     To a release (`version`, of the app's own package unless `package` names another) or a `bundle`,
    code only: the state stays and is read by the new code. Whoever the package's manifest names
    (`upgrades`: the owner, or its publisher's `MANAGE_APPLICATION` holders), with a step-up over `{
    bundle, migrate, manifest }`; an upgrade key signs in place of it. A refused or failed upgrade
    leaves the app on its release and answers `422 UPGRADE_FAILED` with the app log's error in `details`
    (`MigrationsNotSupported` when `migrate` is named: there are no migrations in this version). `error`
    is the app log's error value (`null` when it ran), free-form: the app's own.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppUpgradeResponse200, Error]
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
    body: AppUpgradeBody,
) -> Response[Union[AppUpgradeResponse200, Error]]:
    """Upgrade an app instance

     To a release (`version`, of the app's own package unless `package` names another) or a `bundle`,
    code only: the state stays and is read by the new code. Whoever the package's manifest names
    (`upgrades`: the owner, or its publisher's `MANAGE_APPLICATION` holders), with a step-up over `{
    bundle, migrate, manifest }`; an upgrade key signs in place of it. A refused or failed upgrade
    leaves the app on its release and answers `422 UPGRADE_FAILED` with the app log's error in `details`
    (`MigrationsNotSupported` when `migrate` is named: there are no migrations in this version). `error`
    is the app log's error value (`null` when it ran), free-form: the app's own.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppUpgradeResponse200, Error]]
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
    body: AppUpgradeBody,
) -> Optional[Union[AppUpgradeResponse200, Error]]:
    """Upgrade an app instance

     To a release (`version`, of the app's own package unless `package` names another) or a `bundle`,
    code only: the state stays and is read by the new code. Whoever the package's manifest names
    (`upgrades`: the owner, or its publisher's `MANAGE_APPLICATION` holders), with a step-up over `{
    bundle, migrate, manifest }`; an upgrade key signs in place of it. A refused or failed upgrade
    leaves the app on its release and answers `422 UPGRADE_FAILED` with the app log's error in `details`
    (`MigrationsNotSupported` when `migrate` is named: there are no migrations in this version). `error`
    is the app log's error value (`null` when it ran), free-form: the app's own.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppUpgradeResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
