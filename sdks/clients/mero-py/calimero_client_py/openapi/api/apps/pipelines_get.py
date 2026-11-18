from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.pipelines_get_response_200 import PipelinesGetResponse200
from typing import cast


def _get_kwargs(
    app: str,
    run: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/pipeline-runs/{run}".format(
            app=app,
            run=run,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, PipelinesGetResponse200]]:
    if response.status_code == 200:
        response_200 = PipelinesGetResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

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
) -> Response[Union[Error, PipelinesGetResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    app: str,
    run: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, PipelinesGetResponse200]]:
    """One pipeline run

     Its record: `state`, `version`, `hash`, `computeBundle`, `inputs`, `params`, per-step `status`,
    `runs`, `outputs`, `result`, `reason` and `cost` (`cost` and `costSoFar` for a run with a cap).
    Sizes, hashes and results only for callers who may list the app's files.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        run (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PipelinesGetResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        run=run,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    run: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, PipelinesGetResponse200]]:
    """One pipeline run

     Its record: `state`, `version`, `hash`, `computeBundle`, `inputs`, `params`, per-step `status`,
    `runs`, `outputs`, `result`, `reason` and `cost` (`cost` and `costSoFar` for a run with a cap).
    Sizes, hashes and results only for callers who may list the app's files.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        run (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PipelinesGetResponse200]
    """

    return sync_detailed(
        app=app,
        run=run,
        client=client,
    ).parsed


async def asyncio_detailed(
    app: str,
    run: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, PipelinesGetResponse200]]:
    """One pipeline run

     Its record: `state`, `version`, `hash`, `computeBundle`, `inputs`, `params`, per-step `status`,
    `runs`, `outputs`, `result`, `reason` and `cost` (`cost` and `costSoFar` for a run with a cap).
    Sizes, hashes and results only for callers who may list the app's files.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        run (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PipelinesGetResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        run=run,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    run: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, PipelinesGetResponse200]]:
    """One pipeline run

     Its record: `state`, `version`, `hash`, `computeBundle`, `inputs`, `params`, per-step `status`,
    `runs`, `outputs`, `result`, `reason` and `cost` (`cost` and `costSoFar` for a run with a cap).
    Sizes, hashes and results only for callers who may list the app's files.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        run (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PipelinesGetResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            run=run,
            client=client,
        )
    ).parsed
