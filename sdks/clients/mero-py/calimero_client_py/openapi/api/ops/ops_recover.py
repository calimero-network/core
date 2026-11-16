from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.ops_recover_body import OpsRecoverBody
from ...models.ops_recover_response_200 import OpsRecoverResponse200
from typing import cast


def _get_kwargs(
    *,
    body: OpsRecoverBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/ops/recover",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, OpsRecoverResponse200]]:
    if response.status_code == 200:
        response_200 = OpsRecoverResponse200.from_dict(response.json())

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

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, OpsRecoverResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    body: OpsRecoverBody,
) -> Response[Union[Error, OpsRecoverResponse200]]:
    """Found the cluster again from its recovery disk

     On the founder started with `--bootstrap --recover-from DIR`, its system state empty: the newest
    `system` file in `DIR/operator/` and the apps' `code` files, each with the data key the operator
    opened from it (hex). Checked as founding from backups is (governance, operator key, no node of the
    old cluster answering), then the system group is seeded; each app waits for `POST
    /ops/apps/{app}/restore { recovery }`.

    **Who:** the ops token.

    Args:
        body (OpsRecoverBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsRecoverResponse200]]
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
    body: OpsRecoverBody,
) -> Optional[Union[Error, OpsRecoverResponse200]]:
    """Found the cluster again from its recovery disk

     On the founder started with `--bootstrap --recover-from DIR`, its system state empty: the newest
    `system` file in `DIR/operator/` and the apps' `code` files, each with the data key the operator
    opened from it (hex). Checked as founding from backups is (governance, operator key, no node of the
    old cluster answering), then the system group is seeded; each app waits for `POST
    /ops/apps/{app}/restore { recovery }`.

    **Who:** the ops token.

    Args:
        body (OpsRecoverBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsRecoverResponse200]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    body: OpsRecoverBody,
) -> Response[Union[Error, OpsRecoverResponse200]]:
    """Found the cluster again from its recovery disk

     On the founder started with `--bootstrap --recover-from DIR`, its system state empty: the newest
    `system` file in `DIR/operator/` and the apps' `code` files, each with the data key the operator
    opened from it (hex). Checked as founding from backups is (governance, operator key, no node of the
    old cluster answering), then the system group is seeded; each app waits for `POST
    /ops/apps/{app}/restore { recovery }`.

    **Who:** the ops token.

    Args:
        body (OpsRecoverBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, OpsRecoverResponse200]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    body: OpsRecoverBody,
) -> Optional[Union[Error, OpsRecoverResponse200]]:
    """Found the cluster again from its recovery disk

     On the founder started with `--bootstrap --recover-from DIR`, its system state empty: the newest
    `system` file in `DIR/operator/` and the apps' `code` files, each with the data key the operator
    opened from it (hex). Checked as founding from backups is (governance, operator key, no node of the
    old cluster answering), then the system group is seeded; each app waits for `POST
    /ops/apps/{app}/restore { recovery }`.

    **Who:** the ops token.

    Args:
        body (OpsRecoverBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, OpsRecoverResponse200]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
