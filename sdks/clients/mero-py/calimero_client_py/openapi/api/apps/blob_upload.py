from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.blob_upload_response_200 import BlobUploadResponse200
from ...models.error import Error
from ...types import File, FileTypes
from ...types import UNSET, Unset
from io import BytesIO
from typing import cast
from typing import Union


def _get_kwargs(
    app: str,
    *,
    body: Union[
        File,
        File,
    ],
    sealed: Union[Unset, bool] = UNSET,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    params: dict[str, Any] = {}

    params["sealed"] = sealed

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/blobs".format(
            app=app,
        ),
        "params": params,
    }

    if isinstance(body, File):
        _kwargs["content"] = body.payload

        headers["Content-Type"] = "application/octet-stream"
    if isinstance(body, File):
        _kwargs["content"] = body.payload

        headers["Content-Type"] = "*/*"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[BlobUploadResponse200, Error]]:
    if response.status_code == 200:
        response_200 = BlobUploadResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 400:
        response_400 = Error.from_dict(response.json())

        return response_400

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
) -> Response[Union[BlobUploadResponse200, Error]]:
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
    body: Union[
        File,
        File,
    ],
    sealed: Union[Unset, bool] = UNSET,
) -> Response[Union[BlobUploadResponse200, Error]]:
    """Upload a blob

     A plain blob, stored with the request's `Content-Type`, which a component app's download answers
    with: any media type (printable ASCII, 127 bytes at most; another is `400 BAD_REQUEST`),
    `application/octet-stream` when the request names none. Send the blob's own type, not `*/*`, which
    would be stored as its type. Any `sealed` parameter, `true` or `false`, is refused `400
    USE_FILES_API`: sealed files go through `/files`.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        sealed (Union[Unset, bool]):
        body (File):
        body (File):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[BlobUploadResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
        body=body,
        sealed=sealed,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    *,
    client: AuthenticatedClient,
    body: Union[
        File,
        File,
    ],
    sealed: Union[Unset, bool] = UNSET,
) -> Optional[Union[BlobUploadResponse200, Error]]:
    """Upload a blob

     A plain blob, stored with the request's `Content-Type`, which a component app's download answers
    with: any media type (printable ASCII, 127 bytes at most; another is `400 BAD_REQUEST`),
    `application/octet-stream` when the request names none. Send the blob's own type, not `*/*`, which
    would be stored as its type. Any `sealed` parameter, `true` or `false`, is refused `400
    USE_FILES_API`: sealed files go through `/files`.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        sealed (Union[Unset, bool]):
        body (File):
        body (File):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[BlobUploadResponse200, Error]
    """

    return sync_detailed(
        app=app,
        client=client,
        body=body,
        sealed=sealed,
    ).parsed


async def asyncio_detailed(
    app: str,
    *,
    client: AuthenticatedClient,
    body: Union[
        File,
        File,
    ],
    sealed: Union[Unset, bool] = UNSET,
) -> Response[Union[BlobUploadResponse200, Error]]:
    """Upload a blob

     A plain blob, stored with the request's `Content-Type`, which a component app's download answers
    with: any media type (printable ASCII, 127 bytes at most; another is `400 BAD_REQUEST`),
    `application/octet-stream` when the request names none. Send the blob's own type, not `*/*`, which
    would be stored as its type. Any `sealed` parameter, `true` or `false`, is refused `400
    USE_FILES_API`: sealed files go through `/files`.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        sealed (Union[Unset, bool]):
        body (File):
        body (File):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[BlobUploadResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
        body=body,
        sealed=sealed,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    *,
    client: AuthenticatedClient,
    body: Union[
        File,
        File,
    ],
    sealed: Union[Unset, bool] = UNSET,
) -> Optional[Union[BlobUploadResponse200, Error]]:
    """Upload a blob

     A plain blob, stored with the request's `Content-Type`, which a component app's download answers
    with: any media type (printable ASCII, 127 bytes at most; another is `400 BAD_REQUEST`),
    `application/octet-stream` when the request names none. Send the blob's own type, not `*/*`, which
    would be stored as its type. Any `sealed` parameter, `true` or `false`, is refused `400
    USE_FILES_API`: sealed files go through `/files`.

    **Who:** whoever the app's access admits (a public app: anyone).

    Args:
        app (str):
        sealed (Union[Unset, bool]):
        body (File):
        body (File):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[BlobUploadResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            client=client,
            body=body,
            sealed=sealed,
        )
    ).parsed
