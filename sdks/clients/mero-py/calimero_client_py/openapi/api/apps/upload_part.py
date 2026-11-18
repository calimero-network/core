from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.upload_part_response_200 import UploadPartResponse200
from ...types import File, FileTypes
from io import BytesIO
from typing import cast


def _get_kwargs(
    app: str,
    upload: str,
    k: str,
    *,
    body: File,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "put",
        "url": "/apps/{app}/files/uploads/{upload}/parts/{k}".format(
            app=app,
            upload=upload,
            k=k,
        ),
    }

    _kwargs["content"] = body.payload

    headers["Content-Type"] = "application/octet-stream"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, UploadPartResponse200]]:
    if response.status_code == 200:
        response_200 = UploadPartResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = Error.from_dict(response.json())

        return response_400

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 409:
        response_409 = Error.from_dict(response.json())

        return response_409

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if response.status_code == 503:
        response_503 = Error.from_dict(response.json())

        return response_503

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, UploadPartResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    app: str,
    upload: str,
    k: str,
    *,
    client: AuthenticatedClient,
    body: File,
) -> Response[Union[Error, UploadPartResponse200]]:
    """Send one part

     Part `k` (from 0), raw bytes; a part out of order (`409 PART_OUT_OF_ORDER`), or one that differs
    from the part sent before as `k` (`409 PART_CONFLICT`), answers the next one expected in
    `details.next`.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        upload (str):
        k (str):
        body (File):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, UploadPartResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        upload=upload,
        k=k,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    upload: str,
    k: str,
    *,
    client: AuthenticatedClient,
    body: File,
) -> Optional[Union[Error, UploadPartResponse200]]:
    """Send one part

     Part `k` (from 0), raw bytes; a part out of order (`409 PART_OUT_OF_ORDER`), or one that differs
    from the part sent before as `k` (`409 PART_CONFLICT`), answers the next one expected in
    `details.next`.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        upload (str):
        k (str):
        body (File):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, UploadPartResponse200]
    """

    return sync_detailed(
        app=app,
        upload=upload,
        k=k,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    app: str,
    upload: str,
    k: str,
    *,
    client: AuthenticatedClient,
    body: File,
) -> Response[Union[Error, UploadPartResponse200]]:
    """Send one part

     Part `k` (from 0), raw bytes; a part out of order (`409 PART_OUT_OF_ORDER`), or one that differs
    from the part sent before as `k` (`409 PART_CONFLICT`), answers the next one expected in
    `details.next`.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        upload (str):
        k (str):
        body (File):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, UploadPartResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        upload=upload,
        k=k,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    upload: str,
    k: str,
    *,
    client: AuthenticatedClient,
    body: File,
) -> Optional[Union[Error, UploadPartResponse200]]:
    """Send one part

     Part `k` (from 0), raw bytes; a part out of order (`409 PART_OUT_OF_ORDER`), or one that differs
    from the part sent before as `k` (`409 PART_CONFLICT`), answers the next one expected in
    `details.next`.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        upload (str):
        k (str):
        body (File):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, UploadPartResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            upload=upload,
            k=k,
            client=client,
            body=body,
        )
    ).parsed
