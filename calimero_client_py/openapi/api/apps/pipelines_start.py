from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.pipelines_start_body import PipelinesStartBody
from ...models.pipelines_start_response_200 import PipelinesStartResponse200
from typing import cast


def _get_kwargs(
    app: str,
    name: str,
    *,
    body: PipelinesStartBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/pipelines/{name}/runs".format(
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
) -> Optional[Union[Error, PipelinesStartResponse200]]:
    if response.status_code == 200:
        response_200 = PipelinesStartResponse200.from_dict(response.json())

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

    if response.status_code == 410:
        response_410 = Error.from_dict(response.json())

        return response_410

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 422:
        response_422 = Error.from_dict(response.json())

        return response_422

    if response.status_code == 429:
        response_429 = Error.from_dict(response.json())

        return response_429

    if response.status_code == 503:
        response_503 = Error.from_dict(response.json())

        return response_503

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, PipelinesStartResponse200]]:
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
    body: PipelinesStartBody,
) -> Response[Union[Error, PipelinesStartResponse200]]:
    """Start a pipeline run

     The owner and the organization's members. Starts a run of a pipeline the app's release registers.
    `maxCost` (micro-units, a decimal string) caps the run, and `worstCase` answers it. `202`.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        name (str):
        body (PipelinesStartBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PipelinesStartResponse200]]
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
    body: PipelinesStartBody,
) -> Optional[Union[Error, PipelinesStartResponse200]]:
    """Start a pipeline run

     The owner and the organization's members. Starts a run of a pipeline the app's release registers.
    `maxCost` (micro-units, a decimal string) caps the run, and `worstCase` answers it. `202`.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        name (str):
        body (PipelinesStartBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PipelinesStartResponse200]
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
    body: PipelinesStartBody,
) -> Response[Union[Error, PipelinesStartResponse200]]:
    """Start a pipeline run

     The owner and the organization's members. Starts a run of a pipeline the app's release registers.
    `maxCost` (micro-units, a decimal string) caps the run, and `worstCase` answers it. `202`.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        name (str):
        body (PipelinesStartBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PipelinesStartResponse200]]
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
    body: PipelinesStartBody,
) -> Optional[Union[Error, PipelinesStartResponse200]]:
    """Start a pipeline run

     The owner and the organization's members. Starts a run of a pipeline the app's release registers.
    `maxCost` (micro-units, a decimal string) caps the run, and `worstCase` answers it. `202`.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        name (str):
        body (PipelinesStartBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PipelinesStartResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            name=name,
            client=client,
            body=body,
        )
    ).parsed
