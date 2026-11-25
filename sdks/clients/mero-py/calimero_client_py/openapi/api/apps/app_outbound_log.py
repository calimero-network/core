from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_outbound_log_response_200 import AppOutboundLogResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "get",
        "url": "/apps/{app}/outbound-log".format(
            app=app,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppOutboundLogResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppOutboundLogResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 403:
        response_403 = Error.from_dict(response.json())

        return response_403

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[AppOutboundLogResponse200, Error]]:
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
) -> Response[Union[AppOutboundLogResponse200, Error]]:
    """An app's outbound requests

     The owner or an organization admin: the outbound HTTPS requests the instance's runs made, sent or
    refused, newest first, at most 100 (`MAX_AUDIT_ENTRIES`) and none older than 7 days
    (`AUDIT_RETENTION_MS`). Each replica keeps those its own runs made, in memory, until it restarts;
    `partial` says a replica did not answer. An entry holds no secret value, body, query string or
    header.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppOutboundLogResponse200, Error]]
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
) -> Optional[Union[AppOutboundLogResponse200, Error]]:
    """An app's outbound requests

     The owner or an organization admin: the outbound HTTPS requests the instance's runs made, sent or
    refused, newest first, at most 100 (`MAX_AUDIT_ENTRIES`) and none older than 7 days
    (`AUDIT_RETENTION_MS`). Each replica keeps those its own runs made, in memory, until it restarts;
    `partial` says a replica did not answer. An entry holds no secret value, body, query string or
    header.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppOutboundLogResponse200, Error]
    """

    return sync_detailed(
        app=app,
        client=client,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[AppOutboundLogResponse200, Error]]:
    """An app's outbound requests

     The owner or an organization admin: the outbound HTTPS requests the instance's runs made, sent or
    refused, newest first, at most 100 (`MAX_AUDIT_ENTRIES`) and none older than 7 days
    (`AUDIT_RETENTION_MS`). Each replica keeps those its own runs made, in memory, until it restarts;
    `partial` says a replica did not answer. An entry holds no secret value, body, query string or
    header.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppOutboundLogResponse200, Error]]
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
) -> Optional[Union[AppOutboundLogResponse200, Error]]:
    """An app's outbound requests

     The owner or an organization admin: the outbound HTTPS requests the instance's runs made, sent or
    refused, newest first, at most 100 (`MAX_AUDIT_ENTRIES`) and none older than 7 days
    (`AUDIT_RETENTION_MS`). Each replica keeps those its own runs made, in memory, until it restarts;
    `partial` says a replica did not answer. An entry holds no secret value, body, query string or
    header.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppOutboundLogResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
        )
    ).parsed
