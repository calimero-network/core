from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.intents_body import IntentsBody
from ...models.intents_response_200 import IntentsResponse200
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: IntentsBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/intents".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, IntentsResponse200]]:
    if response.status_code == 200:
        response_200 = IntentsResponse200.from_dict(response.json())

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

    if response.status_code == 412:
        response_412 = Error.from_dict(response.json())

        return response_412

    if response.status_code == 413:
        response_413 = Error.from_dict(response.json())

        return response_413

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
) -> Response[Union[Error, IntentsResponse200]]:
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
    body: IntentsBody,
) -> Response[Union[Error, IntentsResponse200]]:
    """Call a method

     A logged call: the result, the events of the call the caller receives, and the answering node's
    receipt. A `confirm` method answers `412` first. An app error is `422 APP_ERROR` with `details {
    index, code, fields }`; a trap `422 TRAP`. `args`, `result` and each event's `data` are the app's
    own JSON, free-form here: the app's ABI (`GET /apps/{app}/abi`, or its OpenAPI document) types them.

    **Who:** whoever the app's access admits (a public app: anyone). **Step-up:** when the method is a
    `confirm` method of the app's ABI, answers `412 CONFIRM_REQUIRED` first; send the same request again
    with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        app (str):
        body (IntentsBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, IntentsResponse200]]
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
    body: IntentsBody,
) -> Optional[Union[Error, IntentsResponse200]]:
    """Call a method

     A logged call: the result, the events of the call the caller receives, and the answering node's
    receipt. A `confirm` method answers `412` first. An app error is `422 APP_ERROR` with `details {
    index, code, fields }`; a trap `422 TRAP`. `args`, `result` and each event's `data` are the app's
    own JSON, free-form here: the app's ABI (`GET /apps/{app}/abi`, or its OpenAPI document) types them.

    **Who:** whoever the app's access admits (a public app: anyone). **Step-up:** when the method is a
    `confirm` method of the app's ABI, answers `412 CONFIRM_REQUIRED` first; send the same request again
    with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        app (str):
        body (IntentsBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, IntentsResponse200]
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
    body: IntentsBody,
) -> Response[Union[Error, IntentsResponse200]]:
    """Call a method

     A logged call: the result, the events of the call the caller receives, and the answering node's
    receipt. A `confirm` method answers `412` first. An app error is `422 APP_ERROR` with `details {
    index, code, fields }`; a trap `422 TRAP`. `args`, `result` and each event's `data` are the app's
    own JSON, free-form here: the app's ABI (`GET /apps/{app}/abi`, or its OpenAPI document) types them.

    **Who:** whoever the app's access admits (a public app: anyone). **Step-up:** when the method is a
    `confirm` method of the app's ABI, answers `412 CONFIRM_REQUIRED` first; send the same request again
    with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        app (str):
        body (IntentsBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, IntentsResponse200]]
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
    body: IntentsBody,
) -> Optional[Union[Error, IntentsResponse200]]:
    """Call a method

     A logged call: the result, the events of the call the caller receives, and the answering node's
    receipt. A `confirm` method answers `412` first. An app error is `422 APP_ERROR` with `details {
    index, code, fields }`; a trap `422 TRAP`. `args`, `result` and each event's `data` are the app's
    own JSON, free-form here: the app's ABI (`GET /apps/{app}/abi`, or its OpenAPI document) types them.

    **Who:** whoever the app's access admits (a public app: anyone). **Step-up:** when the method is a
    `confirm` method of the app's ABI, answers `412 CONFIRM_REQUIRED` first; send the same request again
    with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        app (str):
        body (IntentsBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, IntentsResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
