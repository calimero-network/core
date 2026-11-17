from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_upgrade_unit_body import AppUpgradeUnitBody
from ...models.error import Error
from ...models.upgrade_run import UpgradeRun
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: AppUpgradeUnitBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/upgrade-unit".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, UpgradeRun]]:
    if response.status_code == 200:
        response_200 = UpgradeRun.from_dict(response.json())

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
) -> Response[Union[Error, UpgradeRun]]:
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
    body: AppUpgradeUnitBody,
) -> Response[Union[Error, UpgradeRun]]:
    """Upgrade an app and the apps it created, in one consent

     An instance created with apps of its own (`links`): it moves to `version` of its package, and each
    member the target release's `uses` gives a release other than the one it runs moves to that release
    (its lock's, else the newest installed one inside the range), all under one step-up over `{ app,
    package, version, targets }`. Answers the run (`GET /orgs/{org}/upgrades/{run}` follows it); each
    app is upgraded on its own as an organization's run does. `400 NOT_A_UNIT` for an app with no
    members; `409 CONFLICT` when the target release creates a member the instance lacks.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppUpgradeUnitBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, UpgradeRun]]
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
    body: AppUpgradeUnitBody,
) -> Optional[Union[Error, UpgradeRun]]:
    """Upgrade an app and the apps it created, in one consent

     An instance created with apps of its own (`links`): it moves to `version` of its package, and each
    member the target release's `uses` gives a release other than the one it runs moves to that release
    (its lock's, else the newest installed one inside the range), all under one step-up over `{ app,
    package, version, targets }`. Answers the run (`GET /orgs/{org}/upgrades/{run}` follows it); each
    app is upgraded on its own as an organization's run does. `400 NOT_A_UNIT` for an app with no
    members; `409 CONFLICT` when the target release creates a member the instance lacks.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppUpgradeUnitBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, UpgradeRun]
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
    body: AppUpgradeUnitBody,
) -> Response[Union[Error, UpgradeRun]]:
    """Upgrade an app and the apps it created, in one consent

     An instance created with apps of its own (`links`): it moves to `version` of its package, and each
    member the target release's `uses` gives a release other than the one it runs moves to that release
    (its lock's, else the newest installed one inside the range), all under one step-up over `{ app,
    package, version, targets }`. Answers the run (`GET /orgs/{org}/upgrades/{run}` follows it); each
    app is upgraded on its own as an organization's run does. `400 NOT_A_UNIT` for an app with no
    members; `409 CONFLICT` when the target release creates a member the instance lacks.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppUpgradeUnitBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, UpgradeRun]]
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
    body: AppUpgradeUnitBody,
) -> Optional[Union[Error, UpgradeRun]]:
    """Upgrade an app and the apps it created, in one consent

     An instance created with apps of its own (`links`): it moves to `version` of its package, and each
    member the target release's `uses` gives a release other than the one it runs moves to that release
    (its lock's, else the newest installed one inside the range), all under one step-up over `{ app,
    package, version, targets }`. Answers the run (`GET /orgs/{org}/upgrades/{run}` follows it); each
    app is upgraded on its own as an organization's run does. `400 NOT_A_UNIT` for an app with no
    members; `409 CONFLICT` when the target release creates a member the instance lacks.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppUpgradeUnitBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, UpgradeRun]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
