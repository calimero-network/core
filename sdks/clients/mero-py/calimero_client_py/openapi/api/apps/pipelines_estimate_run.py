from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.pipelines_estimate_run_body import PipelinesEstimateRunBody
from ...models.pipelines_estimate_run_response_200 import (
    PipelinesEstimateRunResponse200,
)
from typing import cast


def _get_kwargs(
    app: str,
    name: str,
    *,
    body: PipelinesEstimateRunBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/pipelines/{name}/estimate".format(
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
) -> Optional[Union[Error, PipelinesEstimateRunResponse200]]:
    if response.status_code == 200:
        response_200 = PipelinesEstimateRunResponse200.from_dict(response.json())

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

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, PipelinesEstimateRunResponse200]]:
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
    body: PipelinesEstimateRunBody,
) -> Response[Union[Error, PipelinesEstimateRunResponse200]]:
    """Estimate a pipeline run

     The owner and the organization's members. Runs, fuel and cost of a run of a pipeline the app's
    release registers, for `inputs` (file ids, only counted) or `counts`, not both; numbers as decimal
    strings.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        name (str):
        body (PipelinesEstimateRunBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PipelinesEstimateRunResponse200]]
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
    body: PipelinesEstimateRunBody,
) -> Optional[Union[Error, PipelinesEstimateRunResponse200]]:
    """Estimate a pipeline run

     The owner and the organization's members. Runs, fuel and cost of a run of a pipeline the app's
    release registers, for `inputs` (file ids, only counted) or `counts`, not both; numbers as decimal
    strings.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        name (str):
        body (PipelinesEstimateRunBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PipelinesEstimateRunResponse200]
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
    body: PipelinesEstimateRunBody,
) -> Response[Union[Error, PipelinesEstimateRunResponse200]]:
    """Estimate a pipeline run

     The owner and the organization's members. Runs, fuel and cost of a run of a pipeline the app's
    release registers, for `inputs` (file ids, only counted) or `counts`, not both; numbers as decimal
    strings.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        name (str):
        body (PipelinesEstimateRunBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PipelinesEstimateRunResponse200]]
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
    body: PipelinesEstimateRunBody,
) -> Optional[Union[Error, PipelinesEstimateRunResponse200]]:
    """Estimate a pipeline run

     The owner and the organization's members. Runs, fuel and cost of a run of a pipeline the app's
    release registers, for `inputs` (file ids, only counted) or `counts`, not both; numbers as decimal
    strings.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        name (str):
        body (PipelinesEstimateRunBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PipelinesEstimateRunResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            name=name,
            client=client,
            body=body,
        )
    ).parsed
