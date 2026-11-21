from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.rollout_pause_body import RolloutPauseBody
from ...models.rollout_pause_response_200 import RolloutPauseResponse200
from typing import cast


def _get_kwargs(
    id: str,
    app: str,
    *,
    body: RolloutPauseBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/deployments/{id}/rollout/{app}/pause".format(
            id=id,
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, RolloutPauseResponse200]]:
    if response.status_code == 200:
        response_200 = RolloutPauseResponse200.from_dict(response.json())

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
) -> Response[Union[Error, RolloutPauseResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: str,
    app: str,
    *,
    client: AuthenticatedClient,
    body: RolloutPauseBody,
) -> Response[Union[Error, RolloutPauseResponse200]]:
    """Pause a deployment's rollout

     A manager, with a step-up over `deployment:<id> / __set_rollout / { app, paused: true, waveHours }`:
    no more of its instances move to the offered release on their own; the waves open stay open.
    `waveHours` (1 to 168, `400 BAD_ROLLOUT`) sets the pause between waves. `409 FEATURE_NOT_ACTIVE`
    below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        app (str):
        body (RolloutPauseBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, RolloutPauseResponse200]]
    """

    kwargs = _get_kwargs(
        id=id,
        app=app,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: str,
    app: str,
    *,
    client: AuthenticatedClient,
    body: RolloutPauseBody,
) -> Optional[Union[Error, RolloutPauseResponse200]]:
    """Pause a deployment's rollout

     A manager, with a step-up over `deployment:<id> / __set_rollout / { app, paused: true, waveHours }`:
    no more of its instances move to the offered release on their own; the waves open stay open.
    `waveHours` (1 to 168, `400 BAD_ROLLOUT`) sets the pause between waves. `409 FEATURE_NOT_ACTIVE`
    below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        app (str):
        body (RolloutPauseBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, RolloutPauseResponse200]
    """

    return sync_detailed(
        id=id,
        app=app,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    id: str,
    app: str,
    *,
    client: AuthenticatedClient,
    body: RolloutPauseBody,
) -> Response[Union[Error, RolloutPauseResponse200]]:
    """Pause a deployment's rollout

     A manager, with a step-up over `deployment:<id> / __set_rollout / { app, paused: true, waveHours }`:
    no more of its instances move to the offered release on their own; the waves open stay open.
    `waveHours` (1 to 168, `400 BAD_ROLLOUT`) sets the pause between waves. `409 FEATURE_NOT_ACTIVE`
    below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        app (str):
        body (RolloutPauseBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, RolloutPauseResponse200]]
    """

    kwargs = _get_kwargs(
        id=id,
        app=app,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: str,
    app: str,
    *,
    client: AuthenticatedClient,
    body: RolloutPauseBody,
) -> Optional[Union[Error, RolloutPauseResponse200]]:
    """Pause a deployment's rollout

     A manager, with a step-up over `deployment:<id> / __set_rollout / { app, paused: true, waveHours }`:
    no more of its instances move to the offered release on their own; the waves open stay open.
    `waveHours` (1 to 168, `400 BAD_ROLLOUT`) sets the pause between waves. `409 FEATURE_NOT_ACTIVE`
    below cluster version 1.0.0-rc.34.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        app (str):
        body (RolloutPauseBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, RolloutPauseResponse200]
    """

    return (
        await asyncio_detailed(
            id=id,
            app=app,
            client=client,
            body=body,
        )
    ).parsed
