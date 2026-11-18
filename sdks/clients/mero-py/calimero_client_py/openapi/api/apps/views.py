from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.views_body import ViewsBody
from ...models.views_response_200 import ViewsResponse200
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: ViewsBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/views".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, ViewsResponse200]]:
    if response.status_code == 200:
        response_200 = ViewsResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 422:
        response_422 = Error.from_dict(response.json())

        return response_422

    if response.status_code == 503:
        response_503 = Error.from_dict(response.json())

        return response_503

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, ViewsResponse200]]:
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
    body: ViewsBody,
) -> Response[Union[Error, ViewsResponse200]]:
    r"""Read with a view method

     Read linearizably (`consistency: \"strong\"`, the default; any value but `\"stale\"` reads as it):
    the replica answering waits until it applied the index the app's leader names once it confirmed it
    leads, and sends the view to the leader if it cannot catch up; or (`\"stale\"`) on the replica
    answering, as it stands; `minIndex` waits until the replica applied it (`503 NOT_CAUGHT_UP`).
    `args`, `result` and each event's `data` are the app's own JSON, free-form here: the app's ABI (`GET
    /apps/{app}/abi`, or its OpenAPI document) types them.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        body (ViewsBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, ViewsResponse200]]
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
    body: ViewsBody,
) -> Optional[Union[Error, ViewsResponse200]]:
    r"""Read with a view method

     Read linearizably (`consistency: \"strong\"`, the default; any value but `\"stale\"` reads as it):
    the replica answering waits until it applied the index the app's leader names once it confirmed it
    leads, and sends the view to the leader if it cannot catch up; or (`\"stale\"`) on the replica
    answering, as it stands; `minIndex` waits until the replica applied it (`503 NOT_CAUGHT_UP`).
    `args`, `result` and each event's `data` are the app's own JSON, free-form here: the app's ABI (`GET
    /apps/{app}/abi`, or its OpenAPI document) types them.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        body (ViewsBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, ViewsResponse200]
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
    body: ViewsBody,
) -> Response[Union[Error, ViewsResponse200]]:
    r"""Read with a view method

     Read linearizably (`consistency: \"strong\"`, the default; any value but `\"stale\"` reads as it):
    the replica answering waits until it applied the index the app's leader names once it confirmed it
    leads, and sends the view to the leader if it cannot catch up; or (`\"stale\"`) on the replica
    answering, as it stands; `minIndex` waits until the replica applied it (`503 NOT_CAUGHT_UP`).
    `args`, `result` and each event's `data` are the app's own JSON, free-form here: the app's ABI (`GET
    /apps/{app}/abi`, or its OpenAPI document) types them.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        body (ViewsBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, ViewsResponse200]]
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
    body: ViewsBody,
) -> Optional[Union[Error, ViewsResponse200]]:
    r"""Read with a view method

     Read linearizably (`consistency: \"strong\"`, the default; any value but `\"stale\"` reads as it):
    the replica answering waits until it applied the index the app's leader names once it confirmed it
    leads, and sends the view to the leader if it cannot catch up; or (`\"stale\"`) on the replica
    answering, as it stands; `minIndex` waits until the replica applied it (`503 NOT_CAUGHT_UP`).
    `args`, `result` and each event's `data` are the app's own JSON, free-form here: the app's ABI (`GET
    /apps/{app}/abi`, or its OpenAPI document) types them.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        body (ViewsBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, ViewsResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
