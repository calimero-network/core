from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.deployment_body import DeploymentBody
from ...models.deployment_created import DeploymentCreated
from ...models.error import Error
from typing import cast


def _get_kwargs(
    *,
    body: DeploymentBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/deployments",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[DeploymentCreated, Error]]:
    if response.status_code == 200:
        response_200 = DeploymentCreated.from_dict(response.json())

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
) -> Response[Union[DeploymentCreated, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    body: DeploymentBody,
) -> Response[Union[DeploymentCreated, Error]]:
    """Create a deployment

     A named set of releases an owner (an account or organization) offers for others to instantiate.
    Answers its id, the record as made (no apps yet) and its frontend host (`null` on a node with no
    apps domain). An organization owns a deployment only holding a recovery key (`POST
    /orgs/{org}/recovery`), so a lost passkey never locks it: `409 RECOVERY_KEY_REQUIRED` before the
    step-up from cluster version 1.0.0-rc.38.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        body (DeploymentBody): The request body of `POST /deployments`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[DeploymentCreated, Error]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    body: DeploymentBody,
) -> Optional[Union[DeploymentCreated, Error]]:
    """Create a deployment

     A named set of releases an owner (an account or organization) offers for others to instantiate.
    Answers its id, the record as made (no apps yet) and its frontend host (`null` on a node with no
    apps domain). An organization owns a deployment only holding a recovery key (`POST
    /orgs/{org}/recovery`), so a lost passkey never locks it: `409 RECOVERY_KEY_REQUIRED` before the
    step-up from cluster version 1.0.0-rc.38.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        body (DeploymentBody): The request body of `POST /deployments`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[DeploymentCreated, Error]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    body: DeploymentBody,
) -> Response[Union[DeploymentCreated, Error]]:
    """Create a deployment

     A named set of releases an owner (an account or organization) offers for others to instantiate.
    Answers its id, the record as made (no apps yet) and its frontend host (`null` on a node with no
    apps domain). An organization owns a deployment only holding a recovery key (`POST
    /orgs/{org}/recovery`), so a lost passkey never locks it: `409 RECOVERY_KEY_REQUIRED` before the
    step-up from cluster version 1.0.0-rc.38.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        body (DeploymentBody): The request body of `POST /deployments`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[DeploymentCreated, Error]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    body: DeploymentBody,
) -> Optional[Union[DeploymentCreated, Error]]:
    """Create a deployment

     A named set of releases an owner (an account or organization) offers for others to instantiate.
    Answers its id, the record as made (no apps yet) and its frontend host (`null` on a node with no
    apps domain). An organization owns a deployment only holding a recovery key (`POST
    /orgs/{org}/recovery`), so a lost passkey never locks it: `409 RECOVERY_KEY_REQUIRED` before the
    step-up from cluster version 1.0.0-rc.38.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        body (DeploymentBody): The request body of `POST /deployments`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[DeploymentCreated, Error]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
