from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.health_response_200 import HealthResponse200
from typing import cast


def _get_kwargs() -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/health",
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[HealthResponse200]:
    if response.status_code == 200:
        response_200 = HealthResponse200.from_dict(response.json())

        return response_200

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[HealthResponse200]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[HealthResponse200]:
    """Node health

     In the clear. The node's id, the system group's leader (`null`: none known), its `merod` release,
    the newest cluster version it supports (`clusterVersion`) and the cluster's rollback epoch (`null`
    on a node without a rollback counter).

    **Who:** anyone.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[HealthResponse200]
    """

    kwargs = _get_kwargs()

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[HealthResponse200]:
    """Node health

     In the clear. The node's id, the system group's leader (`null`: none known), its `merod` release,
    the newest cluster version it supports (`clusterVersion`) and the cluster's rollback epoch (`null`
    on a node without a rollback counter).

    **Who:** anyone.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        HealthResponse200
    """

    return sync_detailed(
        client=client,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[HealthResponse200]:
    """Node health

     In the clear. The node's id, the system group's leader (`null`: none known), its `merod` release,
    the newest cluster version it supports (`clusterVersion`) and the cluster's rollback epoch (`null`
    on a node without a rollback counter).

    **Who:** anyone.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[HealthResponse200]
    """

    kwargs = _get_kwargs()

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[HealthResponse200]:
    """Node health

     In the clear. The node's id, the system group's leader (`null`: none known), its `merod` release,
    the newest cluster version it supports (`clusterVersion`) and the cluster's rollback epoch (`null`
    on a node without a rollback counter).

    **Who:** anyone.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        HealthResponse200
    """

    return (
        await asyncio_detailed(
            client=client,
        )
    ).parsed
