from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.ops_upgrade_body import OpsUpgradeBody
from ...models.ops_upgrade_response_200 import OpsUpgradeResponse200
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: OpsUpgradeBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/ops/apps/{app}/upgrade".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OpsUpgradeResponse200]]:
    if response.status_code == 200:
        response_200 = OpsUpgradeResponse200.from_dict(response.json())

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

    if response.status_code == 422:
        response_422 = Error.from_dict(response.json())

        return response_422

    if response.status_code == 428:
        response_428 = Error.from_dict(response.json())

        return response_428

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, OpsUpgradeResponse200]]:
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
    body: OpsUpgradeBody,
) -> Response[Union[Error, OpsUpgradeResponse200]]:
    """Upgrade an app as the operator

     To a release the operator installed, where the operator holds app upgrades. `error` is the app log's
    error value (`null` when it ran), free-form: the app's own.

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        app (str):
        body (OpsUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsUpgradeResponse200]]
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
    body: OpsUpgradeBody,
) -> Optional[Union[Error, OpsUpgradeResponse200]]:
    """Upgrade an app as the operator

     To a release the operator installed, where the operator holds app upgrades. `error` is the app log's
    error value (`null` when it ran), free-form: the app's own.

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        app (str):
        body (OpsUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsUpgradeResponse200]
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
    body: OpsUpgradeBody,
) -> Response[Union[Error, OpsUpgradeResponse200]]:
    """Upgrade an app as the operator

     To a release the operator installed, where the operator holds app upgrades. `error` is the app log's
    error value (`null` when it ran), free-form: the app's own.

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        app (str):
        body (OpsUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsUpgradeResponse200]]
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
    body: OpsUpgradeBody,
) -> Optional[Union[Error, OpsUpgradeResponse200]]:
    """Upgrade an app as the operator

     To a release the operator installed, where the operator holds app upgrades. `error` is the app log's
    error value (`null` when it ran), free-form: the app's own.

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        app (str):
        body (OpsUpgradeBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsUpgradeResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
