from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_openapi_response_200 import AppOpenapiResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/openapi.json".format(
            app=app,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppOpenapiResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppOpenapiResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 404:
        response_404 = Error.from_dict(response.json())

        return response_404

    if response.status_code == 410:
        response_410 = Error.from_dict(response.json())

        return response_410

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 422:
        response_422 = Error.from_dict(response.json())

        return response_422

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[AppOpenapiResponse200, Error]]:
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
) -> Response[Union[AppOpenapiResponse200, Error]]:
    """An app's OpenAPI document

     Generated from the ABI of the bundle the app runs, under the access `/abi` has: one `POST
    /apps/{app}/methods/{method}` per method, with its arguments, result and errors. Free-form here: it
    is a document of its own, which a client reads as OpenAPI.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppOpenapiResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AppOpenapiResponse200, Error]]:
    """An app's OpenAPI document

     Generated from the ABI of the bundle the app runs, under the access `/abi` has: one `POST
    /apps/{app}/methods/{method}` per method, with its arguments, result and errors. Free-form here: it
    is a document of its own, which a client reads as OpenAPI.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppOpenapiResponse200, Error]
    """

    return sync_detailed(
        app=app,
        client=client,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[AppOpenapiResponse200, Error]]:
    """An app's OpenAPI document

     Generated from the ABI of the bundle the app runs, under the access `/abi` has: one `POST
    /apps/{app}/methods/{method}` per method, with its arguments, result and errors. Free-form here: it
    is a document of its own, which a client reads as OpenAPI.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppOpenapiResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[AppOpenapiResponse200, Error]]:
    """An app's OpenAPI document

     Generated from the ABI of the bundle the app runs, under the access `/abi` has: one `POST
    /apps/{app}/methods/{method}` per method, with its arguments, result and errors. Free-form here: it
    is a document of its own, which a client reads as OpenAPI.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppOpenapiResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
        )
    ).parsed
