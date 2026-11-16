from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.ops_set_replicas_body import OpsSetReplicasBody
from ...models.ops_set_replicas_response_200 import OpsSetReplicasResponse200
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: OpsSetReplicasBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/ops/apps/{app}/replicas".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OpsSetReplicasResponse200]]:
    if response.status_code == 200:
        response_200 = OpsSetReplicasResponse200.from_dict(response.json())

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

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 428:
        response_428 = Error.from_dict(response.json())

        return response_428

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, OpsSetReplicasResponse200]]:
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
    body: OpsSetReplicasBody,
) -> Response[Union[Error, OpsSetReplicasResponse200]]:
    """Move an app's replicas

     Signed over `<app> / __set_replicas / { replicas }` (sorted); at least `min(3, nodes)` registered
    nodes, each on a release the app's owner approved. The set it already has answers `unchanged: true`.
    `status` is the app group's Raft status when the move finished (as the leader saw it).

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        app (str):
        body (OpsSetReplicasBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsSetReplicasResponse200]]
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
    body: OpsSetReplicasBody,
) -> Optional[Union[Error, OpsSetReplicasResponse200]]:
    """Move an app's replicas

     Signed over `<app> / __set_replicas / { replicas }` (sorted); at least `min(3, nodes)` registered
    nodes, each on a release the app's owner approved. The set it already has answers `unchanged: true`.
    `status` is the app group's Raft status when the move finished (as the leader saw it).

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        app (str):
        body (OpsSetReplicasBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsSetReplicasResponse200]
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
    body: OpsSetReplicasBody,
) -> Response[Union[Error, OpsSetReplicasResponse200]]:
    """Move an app's replicas

     Signed over `<app> / __set_replicas / { replicas }` (sorted); at least `min(3, nodes)` registered
    nodes, each on a release the app's owner approved. The set it already has answers `unchanged: true`.
    `status` is the app group's Raft status when the move finished (as the leader saw it).

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        app (str):
        body (OpsSetReplicasBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsSetReplicasResponse200]]
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
    body: OpsSetReplicasBody,
) -> Optional[Union[Error, OpsSetReplicasResponse200]]:
    """Move an app's replicas

     Signed over `<app> / __set_replicas / { replicas }` (sorted); at least `min(3, nodes)` registered
    nodes, each on a release the app's owner approved. The set it already has answers `unchanged: true`.
    `status` is the app group's Raft status when the move finished (as the leader saw it).

    **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first;
    send it again with `operatorProof`, the operator key's signature over `details.message` and its
    `nonce`.

    Args:
        app (str):
        body (OpsSetReplicasBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsSetReplicasResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
