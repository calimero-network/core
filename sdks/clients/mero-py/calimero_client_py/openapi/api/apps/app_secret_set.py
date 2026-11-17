from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_secret_set_body import AppSecretSetBody
from ...models.app_secret_set_response_200 import AppSecretSetResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
    name: str,
    *,
    body: AppSecretSetBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "put",
        "url": "/apps/{app}/secrets/{name}".format(
            app=app,
            name=name,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppSecretSetResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppSecretSetResponse200.from_dict(response.json())

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
) -> Response[Union[AppSecretSetResponse200, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    app: str,
    name: str,
    *,
    client: AuthenticatedClient,
    body: AppSecretSetBody,
) -> Response[Union[AppSecretSetResponse200, Error]]:
    """Set an app's own value of a declared secret

     Sent only to the secret's declared hosts, never shown again (`app_secrets` answers its fingerprint),
    kept in the app's log on its replicas, out of the app's own reach. The owner, or whoever manages the
    organization's apps, with a step-up over `{ name, fingerprint }`: the value is never in the consent.
    `404` for a name the release does not declare; 1 to 4,096 bytes (`400 BAD_SECRET`); `409
    CLUSTER_NOT_UPGRADED` while a node runs an earlier release.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        name (str):
        body (AppSecretSetBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppSecretSetResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
        name=name,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    name: str,
    *,
    client: AuthenticatedClient,
    body: AppSecretSetBody,
) -> Optional[Union[AppSecretSetResponse200, Error]]:
    """Set an app's own value of a declared secret

     Sent only to the secret's declared hosts, never shown again (`app_secrets` answers its fingerprint),
    kept in the app's log on its replicas, out of the app's own reach. The owner, or whoever manages the
    organization's apps, with a step-up over `{ name, fingerprint }`: the value is never in the consent.
    `404` for a name the release does not declare; 1 to 4,096 bytes (`400 BAD_SECRET`); `409
    CLUSTER_NOT_UPGRADED` while a node runs an earlier release.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        name (str):
        body (AppSecretSetBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppSecretSetResponse200, Error]
    """

    return sync_detailed(
        app=app,
        name=name,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    app: str,
    name: str,
    *,
    client: AuthenticatedClient,
    body: AppSecretSetBody,
) -> Response[Union[AppSecretSetResponse200, Error]]:
    """Set an app's own value of a declared secret

     Sent only to the secret's declared hosts, never shown again (`app_secrets` answers its fingerprint),
    kept in the app's log on its replicas, out of the app's own reach. The owner, or whoever manages the
    organization's apps, with a step-up over `{ name, fingerprint }`: the value is never in the consent.
    `404` for a name the release does not declare; 1 to 4,096 bytes (`400 BAD_SECRET`); `409
    CLUSTER_NOT_UPGRADED` while a node runs an earlier release.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        name (str):
        body (AppSecretSetBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppSecretSetResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
        name=name,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    name: str,
    *,
    client: AuthenticatedClient,
    body: AppSecretSetBody,
) -> Optional[Union[AppSecretSetResponse200, Error]]:
    """Set an app's own value of a declared secret

     Sent only to the secret's declared hosts, never shown again (`app_secrets` answers its fingerprint),
    kept in the app's log on its replicas, out of the app's own reach. The owner, or whoever manages the
    organization's apps, with a step-up over `{ name, fingerprint }`: the value is never in the consent.
    `404` for a name the release does not declare; 1 to 4,096 bytes (`400 BAD_SECRET`); `409
    CLUSTER_NOT_UPGRADED` while a node runs an earlier release.

    **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers
    `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over
    `details.challenge`.

    Args:
        app (str):
        name (str):
        body (AppSecretSetBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppSecretSetResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            name=name,
            client=client,
            body=body,
        )
    ).parsed
