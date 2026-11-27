from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_restore_consent_body import AppRestoreConsentBody
from ...models.error import Error
from ...models.restore_consent import RestoreConsent
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: AppRestoreConsentBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/restore-consent".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, RestoreConsent]]:
    if response.status_code == 200:
        response_200 = RestoreConsent.from_dict(response.json())

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
) -> Response[Union[Error, RestoreConsent]]:
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
    body: AppRestoreConsentBody,
) -> Response[Union[Error, RestoreConsent]]:
    """Consent to restoring a pinned instance to older code

     An owner or admin of the app's organization, while an undo of its last upgrade is open, with a step-
    up over `<app> / __restore_older / { backup, sha256, bundle, point }` (`bundle`: what the backup's
    state runs; `point`: the restore point the open undo would revert): answers the consent the
    operator's restore (`POST /ops/apps/{app}/restore`, `owner`) carries to restore a pinned instance
    (its record's `lineage`) from that backup to code older than the release it runs. `403 FORBIDDEN`
    for anyone else and `409 UNDO_WINDOW_PASSED` once the undo closed, before the step-up; `409
    FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.39.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppRestoreConsentBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, RestoreConsent]]
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
    body: AppRestoreConsentBody,
) -> Optional[Union[Error, RestoreConsent]]:
    """Consent to restoring a pinned instance to older code

     An owner or admin of the app's organization, while an undo of its last upgrade is open, with a step-
    up over `<app> / __restore_older / { backup, sha256, bundle, point }` (`bundle`: what the backup's
    state runs; `point`: the restore point the open undo would revert): answers the consent the
    operator's restore (`POST /ops/apps/{app}/restore`, `owner`) carries to restore a pinned instance
    (its record's `lineage`) from that backup to code older than the release it runs. `403 FORBIDDEN`
    for anyone else and `409 UNDO_WINDOW_PASSED` once the undo closed, before the step-up; `409
    FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.39.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppRestoreConsentBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, RestoreConsent]
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
    body: AppRestoreConsentBody,
) -> Response[Union[Error, RestoreConsent]]:
    """Consent to restoring a pinned instance to older code

     An owner or admin of the app's organization, while an undo of its last upgrade is open, with a step-
    up over `<app> / __restore_older / { backup, sha256, bundle, point }` (`bundle`: what the backup's
    state runs; `point`: the restore point the open undo would revert): answers the consent the
    operator's restore (`POST /ops/apps/{app}/restore`, `owner`) carries to restore a pinned instance
    (its record's `lineage`) from that backup to code older than the release it runs. `403 FORBIDDEN`
    for anyone else and `409 UNDO_WINDOW_PASSED` once the undo closed, before the step-up; `409
    FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.39.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppRestoreConsentBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, RestoreConsent]]
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
    body: AppRestoreConsentBody,
) -> Optional[Union[Error, RestoreConsent]]:
    """Consent to restoring a pinned instance to older code

     An owner or admin of the app's organization, while an undo of its last upgrade is open, with a step-
    up over `<app> / __restore_older / { backup, sha256, bundle, point }` (`bundle`: what the backup's
    state runs; `point`: the restore point the open undo would revert): answers the consent the
    operator's restore (`POST /ops/apps/{app}/restore`, `owner`) carries to restore a pinned instance
    (its record's `lineage`) from that backup to code older than the release it runs. `403 FORBIDDEN`
    for anyone else and `409 UNDO_WINDOW_PASSED` once the undo closed, before the step-up; `409
    FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.39.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        body (AppRestoreConsentBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, RestoreConsent]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
