from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.pipelines_list_response_200 import PipelinesListResponse200
from ...types import UNSET, Unset
from typing import cast
from typing import Union


def _get_kwargs(
    app: str,
    *,
    limit: Union[Unset, str] = UNSET,
) -> dict[str, Any]:

    params: dict[str, Any] = {}

    params["limit"] = limit

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/pipeline-runs".format(
            app=app,
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, PipelinesListResponse200]]:
    if response.status_code == 200:
        response_200 = PipelinesListResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = Error.from_dict(response.json())

        return response_400

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
) -> Response[Union[Error, PipelinesListResponse200]]:
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
    limit: Union[Unset, str] = UNSET,
) -> Response[Union[Error, PipelinesListResponse200]]:
    """An app's pipeline runs

     The recent runs, newest first, as `{ runs }`; `limit` 1 to 100. Sizes, hashes and results only for
    callers who may list the app's files.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        limit (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PipelinesListResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        limit=limit,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    *,
    client: AuthenticatedClient,
    limit: Union[Unset, str] = UNSET,
) -> Optional[Union[Error, PipelinesListResponse200]]:
    """An app's pipeline runs

     The recent runs, newest first, as `{ runs }`; `limit` 1 to 100. Sizes, hashes and results only for
    callers who may list the app's files.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        limit (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PipelinesListResponse200]
    """

    return sync_detailed(
        app=app,
        client=client,
        limit=limit,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
    limit: Union[Unset, str] = UNSET,
) -> Response[Union[Error, PipelinesListResponse200]]:
    """An app's pipeline runs

     The recent runs, newest first, as `{ runs }`; `limit` 1 to 100. Sizes, hashes and results only for
    callers who may list the app's files.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        limit (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PipelinesListResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        limit=limit,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    *,
    client: AuthenticatedClient,
    limit: Union[Unset, str] = UNSET,
) -> Optional[Union[Error, PipelinesListResponse200]]:
    """An app's pipeline runs

     The recent runs, newest first, as `{ runs }`; `limit` 1 to 100. Sizes, hashes and results only for
    callers who may list the app's files.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        limit (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PipelinesListResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            limit=limit,
        )
    ).parsed
