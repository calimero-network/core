from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.ops_deployer_revoke_post_response_200 import (
    OpsDeployerRevokePostResponse200,
)
from ...models.proof_only import ProofOnly
from typing import cast


def _get_kwargs(
    key: str,
    *,
    body: ProofOnly,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/ops/deployers/{key}/revoke".format(
            key=key,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OpsDeployerRevokePostResponse200]]:
    if response.status_code == 200:
        response_200 = OpsDeployerRevokePostResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if response.status_code == 428:
        response_428 = Error.from_dict(response.json())

        return response_428

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, OpsDeployerRevokePostResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    key: str,
    *,
    client: AuthenticatedClient,
    body: ProofOnly,
) -> Response[Union[Error, OpsDeployerRevokePostResponse200]]:
    """Revoke a deployer (POST)

     The same as the `DELETE`, for an HTTP client whose `DELETE` carries no body.

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        key (str):
        body (ProofOnly): What an operator decision that names nothing in its body may carry: none
            at first, the operator's proof after the `428`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsDeployerRevokePostResponse200]]
    """

    kwargs = _get_kwargs(
        key=key,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    key: str,
    *,
    client: AuthenticatedClient,
    body: ProofOnly,
) -> Optional[Union[Error, OpsDeployerRevokePostResponse200]]:
    """Revoke a deployer (POST)

     The same as the `DELETE`, for an HTTP client whose `DELETE` carries no body.

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        key (str):
        body (ProofOnly): What an operator decision that names nothing in its body may carry: none
            at first, the operator's proof after the `428`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsDeployerRevokePostResponse200]
    """

    return sync_detailed(
        key=key,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    key: str,
    *,
    client: AuthenticatedClient,
    body: ProofOnly,
) -> Response[Union[Error, OpsDeployerRevokePostResponse200]]:
    """Revoke a deployer (POST)

     The same as the `DELETE`, for an HTTP client whose `DELETE` carries no body.

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        key (str):
        body (ProofOnly): What an operator decision that names nothing in its body may carry: none
            at first, the operator's proof after the `428`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsDeployerRevokePostResponse200]]
    """

    kwargs = _get_kwargs(
        key=key,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    key: str,
    *,
    client: AuthenticatedClient,
    body: ProofOnly,
) -> Optional[Union[Error, OpsDeployerRevokePostResponse200]]:
    """Revoke a deployer (POST)

     The same as the `DELETE`, for an HTTP client whose `DELETE` carries no body.

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        key (str):
        body (ProofOnly): What an operator decision that names nothing in its body may carry: none
            at first, the operator's proof after the `428`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsDeployerRevokePostResponse200]
    """

    return (
        await asyncio_detailed(
            key=key,
            client=client,
            body=body,
        )
    ).parsed
