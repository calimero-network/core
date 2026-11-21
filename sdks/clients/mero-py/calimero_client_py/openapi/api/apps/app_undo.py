from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_undo_body import AppUndoBody
from ...models.app_undo_response_200 import AppUndoResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: AppUndoBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/undo".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppUndoResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppUndoResponse200.from_dict(response.json())

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
) -> Response[Union[AppUndoResponse200, Error]]:
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
    body: AppUndoBody,
) -> Response[Union[AppUndoResponse200, Error]]:
    """Undo an app's last upgrade

     An owner or admin of the app's organization, within a week of the upgrade (judged on the logged
    times), with a step-up over `<app> / __undo_upgrade / { apps: { <app>: { to, bundle, from,
    restorePoint } } }`. The app, and the members its unit's upgrade moved, go back to the release they
    ran before and, where the upgrade ran on their group, to their restore point's state in a new group
    (seeded by the node that holds it, as a restore is): the one move to an older release. Whatever they
    wrote since the restore point is lost (`apps[].lostWrites`, as `GET` showed it). Answers once each
    group takes calls again, or after a minute with `restored: false` (calls answer `409 APP_RESTORING`
    meanwhile; sending the undo again seeds it again). `409 NO_RESTORE_POINT`, `UNDO_WINDOW_PASSED`,
    `UNDO_STALE` (the app moved on), `UNDO_NOT_AS_NAMED` (a member its unit's run moved: undo its
    instance), `RESTORE_POINT_UNAVAILABLE` (its node does not answer), each before the step-up; `409
    FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppUndoBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppUndoResponse200, Error]]
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
    body: AppUndoBody,
) -> Optional[Union[AppUndoResponse200, Error]]:
    """Undo an app's last upgrade

     An owner or admin of the app's organization, within a week of the upgrade (judged on the logged
    times), with a step-up over `<app> / __undo_upgrade / { apps: { <app>: { to, bundle, from,
    restorePoint } } }`. The app, and the members its unit's upgrade moved, go back to the release they
    ran before and, where the upgrade ran on their group, to their restore point's state in a new group
    (seeded by the node that holds it, as a restore is): the one move to an older release. Whatever they
    wrote since the restore point is lost (`apps[].lostWrites`, as `GET` showed it). Answers once each
    group takes calls again, or after a minute with `restored: false` (calls answer `409 APP_RESTORING`
    meanwhile; sending the undo again seeds it again). `409 NO_RESTORE_POINT`, `UNDO_WINDOW_PASSED`,
    `UNDO_STALE` (the app moved on), `UNDO_NOT_AS_NAMED` (a member its unit's run moved: undo its
    instance), `RESTORE_POINT_UNAVAILABLE` (its node does not answer), each before the step-up; `409
    FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppUndoBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppUndoResponse200, Error]
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
    body: AppUndoBody,
) -> Response[Union[AppUndoResponse200, Error]]:
    """Undo an app's last upgrade

     An owner or admin of the app's organization, within a week of the upgrade (judged on the logged
    times), with a step-up over `<app> / __undo_upgrade / { apps: { <app>: { to, bundle, from,
    restorePoint } } }`. The app, and the members its unit's upgrade moved, go back to the release they
    ran before and, where the upgrade ran on their group, to their restore point's state in a new group
    (seeded by the node that holds it, as a restore is): the one move to an older release. Whatever they
    wrote since the restore point is lost (`apps[].lostWrites`, as `GET` showed it). Answers once each
    group takes calls again, or after a minute with `restored: false` (calls answer `409 APP_RESTORING`
    meanwhile; sending the undo again seeds it again). `409 NO_RESTORE_POINT`, `UNDO_WINDOW_PASSED`,
    `UNDO_STALE` (the app moved on), `UNDO_NOT_AS_NAMED` (a member its unit's run moved: undo its
    instance), `RESTORE_POINT_UNAVAILABLE` (its node does not answer), each before the step-up; `409
    FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppUndoBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppUndoResponse200, Error]]
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
    body: AppUndoBody,
) -> Optional[Union[AppUndoResponse200, Error]]:
    """Undo an app's last upgrade

     An owner or admin of the app's organization, within a week of the upgrade (judged on the logged
    times), with a step-up over `<app> / __undo_upgrade / { apps: { <app>: { to, bundle, from,
    restorePoint } } }`. The app, and the members its unit's upgrade moved, go back to the release they
    ran before and, where the upgrade ran on their group, to their restore point's state in a new group
    (seeded by the node that holds it, as a restore is): the one move to an older release. Whatever they
    wrote since the restore point is lost (`apps[].lostWrites`, as `GET` showed it). Answers once each
    group takes calls again, or after a minute with `restored: false` (calls answer `409 APP_RESTORING`
    meanwhile; sending the undo again seeds it again). `409 NO_RESTORE_POINT`, `UNDO_WINDOW_PASSED`,
    `UNDO_STALE` (the app moved on), `UNDO_NOT_AS_NAMED` (a member its unit's run moved: undo its
    instance), `RESTORE_POINT_UNAVAILABLE` (its node does not answer), each before the step-up; `409
    FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppUndoBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppUndoResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
