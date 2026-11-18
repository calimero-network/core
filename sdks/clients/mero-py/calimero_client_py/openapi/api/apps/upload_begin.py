from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.upload_begin_body import UploadBeginBody
from ...models.upload_begin_response_200 import UploadBeginResponse200
from typing import cast


def _get_kwargs(
    app: str,
    *,
    body: UploadBeginBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/files/uploads".format(
            app=app,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, UploadBeginResponse200]]:
    if response.status_code == 200:
        response_200 = UploadBeginResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 413:
        response_413 = Error.from_dict(response.json())

        return response_413

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 429:
        response_429 = Error.from_dict(response.json())

        return response_429

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, UploadBeginResponse200]]:
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
    body: UploadBeginBody,
) -> Response[Union[Error, UploadBeginResponse200]]:
    """Begin a file upload

     A session for a file of `size` bytes, sent in parts of `partBytes`.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        body (UploadBeginBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, UploadBeginResponse200]]
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
    body: UploadBeginBody,
) -> Optional[Union[Error, UploadBeginResponse200]]:
    """Begin a file upload

     A session for a file of `size` bytes, sent in parts of `partBytes`.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        body (UploadBeginBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, UploadBeginResponse200]
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
    body: UploadBeginBody,
) -> Response[Union[Error, UploadBeginResponse200]]:
    """Begin a file upload

     A session for a file of `size` bytes, sent in parts of `partBytes`.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        body (UploadBeginBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, UploadBeginResponse200]]
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
    body: UploadBeginBody,
) -> Optional[Union[Error, UploadBeginResponse200]]:
    """Begin a file upload

     A session for a file of `size` bytes, sent in parts of `partBytes`.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        body (UploadBeginBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, UploadBeginResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
        )
    ).parsed
