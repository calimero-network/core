from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.deployment_transfer_accept_body import DeploymentTransferAcceptBody
from ...models.deployment_transfer_accept_response_200 import (
    DeploymentTransferAcceptResponse200,
)
from ...models.error import Error
from typing import cast


def _get_kwargs(
    id: str,
    *,
    body: DeploymentTransferAcceptBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/deployments/{id}/transfer/accept".format(
            id=id,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[DeploymentTransferAcceptResponse200, Error]]:
    if response.status_code == 200:
        response_200 = DeploymentTransferAcceptResponse200.from_dict(response.json())

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

    if response.status_code == 429:
        response_429 = Error.from_dict(response.json())

        return response_429

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[DeploymentTransferAcceptResponse200, Error]]:
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
    body: DeploymentTransferAcceptBody,
) -> Response[Union[DeploymentTransferAcceptResponse200, Error]]:
    """Accept a deployment offered to an owner

     A manager of `to` (the account itself, or `MANAGE_APPLICATION` org-wide in the organization) accepts
    the offer to `to`, with a step-up over `deployment:{id} / __accept_deployment / { to, offeredAt }`
    (the offer it accepts): `to` becomes the owner, and the deployment's offers and customers' instances
    stay with it. The previous owner's secret values are dropped (`secretsDropped` names them): the new
    owner sets its own. Until a person approves an update of a customer's instance on a sheet that names
    the new owner, none applies on its own. Refused before the step-up: `404 NO_TRANSFER` when it is not
    offered to `to`, `409 TRANSFER_STALE` once whoever offered it may no longer hand it over, `409
    TRANSFER_LAPSED` 7 days after the offer, `403 DEPLOY_NOT_GRANTED` where deployments are granted and
    `to` holds no live deployer grant of its own, `409 RECOVERY_KEY_REQUIRED` for an organization
    without a recovery key, `429` past the receiver's 16 deployments. Each customer's next update sheet
    names the new owner.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        body (DeploymentTransferAcceptBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[DeploymentTransferAcceptResponse200, Error]]
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
    body: DeploymentTransferAcceptBody,
) -> Optional[Union[DeploymentTransferAcceptResponse200, Error]]:
    """Accept a deployment offered to an owner

     A manager of `to` (the account itself, or `MANAGE_APPLICATION` org-wide in the organization) accepts
    the offer to `to`, with a step-up over `deployment:{id} / __accept_deployment / { to, offeredAt }`
    (the offer it accepts): `to` becomes the owner, and the deployment's offers and customers' instances
    stay with it. The previous owner's secret values are dropped (`secretsDropped` names them): the new
    owner sets its own. Until a person approves an update of a customer's instance on a sheet that names
    the new owner, none applies on its own. Refused before the step-up: `404 NO_TRANSFER` when it is not
    offered to `to`, `409 TRANSFER_STALE` once whoever offered it may no longer hand it over, `409
    TRANSFER_LAPSED` 7 days after the offer, `403 DEPLOY_NOT_GRANTED` where deployments are granted and
    `to` holds no live deployer grant of its own, `409 RECOVERY_KEY_REQUIRED` for an organization
    without a recovery key, `429` past the receiver's 16 deployments. Each customer's next update sheet
    names the new owner.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        body (DeploymentTransferAcceptBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[DeploymentTransferAcceptResponse200, Error]
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
    body: DeploymentTransferAcceptBody,
) -> Response[Union[DeploymentTransferAcceptResponse200, Error]]:
    """Accept a deployment offered to an owner

     A manager of `to` (the account itself, or `MANAGE_APPLICATION` org-wide in the organization) accepts
    the offer to `to`, with a step-up over `deployment:{id} / __accept_deployment / { to, offeredAt }`
    (the offer it accepts): `to` becomes the owner, and the deployment's offers and customers' instances
    stay with it. The previous owner's secret values are dropped (`secretsDropped` names them): the new
    owner sets its own. Until a person approves an update of a customer's instance on a sheet that names
    the new owner, none applies on its own. Refused before the step-up: `404 NO_TRANSFER` when it is not
    offered to `to`, `409 TRANSFER_STALE` once whoever offered it may no longer hand it over, `409
    TRANSFER_LAPSED` 7 days after the offer, `403 DEPLOY_NOT_GRANTED` where deployments are granted and
    `to` holds no live deployer grant of its own, `409 RECOVERY_KEY_REQUIRED` for an organization
    without a recovery key, `429` past the receiver's 16 deployments. Each customer's next update sheet
    names the new owner.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        body (DeploymentTransferAcceptBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[DeploymentTransferAcceptResponse200, Error]]
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
    body: DeploymentTransferAcceptBody,
) -> Optional[Union[DeploymentTransferAcceptResponse200, Error]]:
    """Accept a deployment offered to an owner

     A manager of `to` (the account itself, or `MANAGE_APPLICATION` org-wide in the organization) accepts
    the offer to `to`, with a step-up over `deployment:{id} / __accept_deployment / { to, offeredAt }`
    (the offer it accepts): `to` becomes the owner, and the deployment's offers and customers' instances
    stay with it. The previous owner's secret values are dropped (`secretsDropped` names them): the new
    owner sets its own. Until a person approves an update of a customer's instance on a sheet that names
    the new owner, none applies on its own. Refused before the step-up: `404 NO_TRANSFER` when it is not
    offered to `to`, `409 TRANSFER_STALE` once whoever offered it may no longer hand it over, `409
    TRANSFER_LAPSED` 7 days after the offer, `403 DEPLOY_NOT_GRANTED` where deployments are granted and
    `to` holds no live deployer grant of its own, `409 RECOVERY_KEY_REQUIRED` for an organization
    without a recovery key, `429` past the receiver's 16 deployments. Each customer's next update sheet
    names the new owner.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        body (DeploymentTransferAcceptBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[DeploymentTransferAcceptResponse200, Error]
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            body=body,
        )
    ).parsed
