from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.deployment_secret_body import DeploymentSecretBody
from ...models.deployment_secret_set import DeploymentSecretSet
from ...models.error import Error
from typing import cast


def _get_kwargs(
    id: str,
    name: str,
    *,
    body: DeploymentSecretBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "put",
        "url": "/deployments/{id}/secrets/{name}".format(
            id=id,
            name=name,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[DeploymentSecretSet, Error]]:
    if response.status_code == 200:
        response_200 = DeploymentSecretSet.from_dict(response.json())

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
) -> Response[Union[DeploymentSecretSet, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: str,
    name: str,
    *,
    client: AuthenticatedClient,
    body: DeploymentSecretBody,
) -> Response[Union[DeploymentSecretSet, Error]]:
    """Set a deployment's value of a secret

     Every instance created from the deployment uses it unless the instance set its own
    (`app_secret_set`), and only where the secret's binding says (its host, header or body field), for
    an instance whose release declares the name; it wins over the operator's. Kept in the system group's
    state, sealed, never shown again (`deployment_secrets` answers its fingerprint) and out of every
    app's reach. A manager of the deployment, with a step-up over `deployment:{id} / __set_secret / {
    name, fingerprint }`: the value is never in the consent. `404 UNDECLARED_SECRET` unless a release
    the deployment offers declares the name, or a release one of those references in its signed `uses`
    (an installed release of the package inside the reference's range), whose value reaches the apps an
    instance creates with it; a name of A-Z, 0-9 and `_` and a value of 1 to 4,096 bytes (`400
    BAD_SECRET`); at most 16 per deployment (`429 QUOTA_EXCEEDED`); `409 FEATURE_NOT_ACTIVE` below
    cluster version 1.0.0-rc.32.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        name (str):
        body (DeploymentSecretBody): The request body of `PUT /deployments/{id}/secrets/{name}`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[DeploymentSecretSet, Error]]
    """

    kwargs = _get_kwargs(
        id=id,
        name=name,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: str,
    name: str,
    *,
    client: AuthenticatedClient,
    body: DeploymentSecretBody,
) -> Optional[Union[DeploymentSecretSet, Error]]:
    """Set a deployment's value of a secret

     Every instance created from the deployment uses it unless the instance set its own
    (`app_secret_set`), and only where the secret's binding says (its host, header or body field), for
    an instance whose release declares the name; it wins over the operator's. Kept in the system group's
    state, sealed, never shown again (`deployment_secrets` answers its fingerprint) and out of every
    app's reach. A manager of the deployment, with a step-up over `deployment:{id} / __set_secret / {
    name, fingerprint }`: the value is never in the consent. `404 UNDECLARED_SECRET` unless a release
    the deployment offers declares the name, or a release one of those references in its signed `uses`
    (an installed release of the package inside the reference's range), whose value reaches the apps an
    instance creates with it; a name of A-Z, 0-9 and `_` and a value of 1 to 4,096 bytes (`400
    BAD_SECRET`); at most 16 per deployment (`429 QUOTA_EXCEEDED`); `409 FEATURE_NOT_ACTIVE` below
    cluster version 1.0.0-rc.32.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        name (str):
        body (DeploymentSecretBody): The request body of `PUT /deployments/{id}/secrets/{name}`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[DeploymentSecretSet, Error]
    """

    return sync_detailed(
        id=id,
        name=name,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    id: str,
    name: str,
    *,
    client: AuthenticatedClient,
    body: DeploymentSecretBody,
) -> Response[Union[DeploymentSecretSet, Error]]:
    """Set a deployment's value of a secret

     Every instance created from the deployment uses it unless the instance set its own
    (`app_secret_set`), and only where the secret's binding says (its host, header or body field), for
    an instance whose release declares the name; it wins over the operator's. Kept in the system group's
    state, sealed, never shown again (`deployment_secrets` answers its fingerprint) and out of every
    app's reach. A manager of the deployment, with a step-up over `deployment:{id} / __set_secret / {
    name, fingerprint }`: the value is never in the consent. `404 UNDECLARED_SECRET` unless a release
    the deployment offers declares the name, or a release one of those references in its signed `uses`
    (an installed release of the package inside the reference's range), whose value reaches the apps an
    instance creates with it; a name of A-Z, 0-9 and `_` and a value of 1 to 4,096 bytes (`400
    BAD_SECRET`); at most 16 per deployment (`429 QUOTA_EXCEEDED`); `409 FEATURE_NOT_ACTIVE` below
    cluster version 1.0.0-rc.32.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        name (str):
        body (DeploymentSecretBody): The request body of `PUT /deployments/{id}/secrets/{name}`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[DeploymentSecretSet, Error]]
    """

    kwargs = _get_kwargs(
        id=id,
        name=name,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: str,
    name: str,
    *,
    client: AuthenticatedClient,
    body: DeploymentSecretBody,
) -> Optional[Union[DeploymentSecretSet, Error]]:
    """Set a deployment's value of a secret

     Every instance created from the deployment uses it unless the instance set its own
    (`app_secret_set`), and only where the secret's binding says (its host, header or body field), for
    an instance whose release declares the name; it wins over the operator's. Kept in the system group's
    state, sealed, never shown again (`deployment_secrets` answers its fingerprint) and out of every
    app's reach. A manager of the deployment, with a step-up over `deployment:{id} / __set_secret / {
    name, fingerprint }`: the value is never in the consent. `404 UNDECLARED_SECRET` unless a release
    the deployment offers declares the name, or a release one of those references in its signed `uses`
    (an installed release of the package inside the reference's range), whose value reaches the apps an
    instance creates with it; a name of A-Z, 0-9 and `_` and a value of 1 to 4,096 bytes (`400
    BAD_SECRET`); at most 16 per deployment (`429 QUOTA_EXCEEDED`); `409 FEATURE_NOT_ACTIVE` below
    cluster version 1.0.0-rc.32.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        id (str):
        name (str):
        body (DeploymentSecretBody): The request body of `PUT /deployments/{id}/secrets/{name}`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[DeploymentSecretSet, Error]
    """

    return (
        await asyncio_detailed(
            id=id,
            name=name,
            client=client,
            body=body,
        )
    ).parsed
