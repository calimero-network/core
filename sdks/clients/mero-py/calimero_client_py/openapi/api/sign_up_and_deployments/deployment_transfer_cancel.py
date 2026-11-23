from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.confirm_only import ConfirmOnly
from ...models.deployment_transfer_cancel_response_200 import (
    DeploymentTransferCancelResponse200,
)
from ...models.error import Error
from typing import cast


def _get_kwargs(
    id: str,
    *,
    body: ConfirmOnly,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "delete",
        "url": "/deployments/{id}/transfer".format(
            id=id,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[DeploymentTransferCancelResponse200, Error]]:
    if response.status_code == 200:
        response_200 = DeploymentTransferCancelResponse200.from_dict(response.json())

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
) -> Response[Union[DeploymentTransferCancelResponse200, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: str,
    *,
    client: AuthenticatedClient,
    body: ConfirmOnly,
) -> Response[Union[DeploymentTransferCancelResponse200, Error]]:
    """Cancel the offer of a deployment

     A manager of its owner, or of the owner it is offered to, drops the offer, with a step-up over
    `deployment:{id} / __cancel_deployment_transfer / {}`. `404 NO_TRANSFER` when none waits.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        body (ConfirmOnly): The body of a step-up that names nothing else: none (or `{}`) at
            first, the passkey assertion after the `412`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[DeploymentTransferCancelResponse200, Error]]
    """

    kwargs = _get_kwargs(
        id=id,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: str,
    *,
    client: AuthenticatedClient,
    body: ConfirmOnly,
) -> Optional[Union[DeploymentTransferCancelResponse200, Error]]:
    """Cancel the offer of a deployment

     A manager of its owner, or of the owner it is offered to, drops the offer, with a step-up over
    `deployment:{id} / __cancel_deployment_transfer / {}`. `404 NO_TRANSFER` when none waits.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        body (ConfirmOnly): The body of a step-up that names nothing else: none (or `{}`) at
            first, the passkey assertion after the `412`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[DeploymentTransferCancelResponse200, Error]
    """

    return sync_detailed(
        id=id,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    id: str,
    *,
    client: AuthenticatedClient,
    body: ConfirmOnly,
) -> Response[Union[DeploymentTransferCancelResponse200, Error]]:
    """Cancel the offer of a deployment

     A manager of its owner, or of the owner it is offered to, drops the offer, with a step-up over
    `deployment:{id} / __cancel_deployment_transfer / {}`. `404 NO_TRANSFER` when none waits.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        body (ConfirmOnly): The body of a step-up that names nothing else: none (or `{}`) at
            first, the passkey assertion after the `412`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[DeploymentTransferCancelResponse200, Error]]
    """

    kwargs = _get_kwargs(
        id=id,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: str,
    *,
    client: AuthenticatedClient,
    body: ConfirmOnly,
) -> Optional[Union[DeploymentTransferCancelResponse200, Error]]:
    """Cancel the offer of a deployment

     A manager of its owner, or of the owner it is offered to, drops the offer, with a step-up over
    `deployment:{id} / __cancel_deployment_transfer / {}`. `404 NO_TRANSFER` when none waits.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        body (ConfirmOnly): The body of a step-up that names nothing else: none (or `{}`) at
            first, the passkey assertion after the `412`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[DeploymentTransferCancelResponse200, Error]
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            body=body,
        )
    ).parsed
