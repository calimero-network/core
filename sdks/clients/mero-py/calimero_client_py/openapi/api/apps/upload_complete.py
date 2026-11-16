from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.upload_complete_response_200 import UploadCompleteResponse200
from typing import cast


def _get_kwargs(
    app: str,
    upload: str,
) -> dict[str, Any]:

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/apps/{app}/files/uploads/{upload}/complete".format(
            app=app,
            upload=upload,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, UploadCompleteResponse200]]:
    if response.status_code == 200:
        response_200 = UploadCompleteResponse200.from_dict(response.json())

        return response_200

    if response.status_code == 401:
        response_401 = Error.from_dict(response.json())

        return response_401

    if response.status_code == 409:
        response_409 = Error.from_dict(response.json())

        return response_409

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Error, UploadCompleteResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    app: str,
    upload: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, UploadCompleteResponse200]]:
    """Commit an upload

     Charges and commits the file; a retry answers the same file. `409 UPLOAD_INCOMPLETE` names the part
    still missing.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        upload (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, UploadCompleteResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        upload=upload,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    upload: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, UploadCompleteResponse200]]:
    """Commit an upload

     Charges and commits the file; a retry answers the same file. `409 UPLOAD_INCOMPLETE` names the part
    still missing.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        upload (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, UploadCompleteResponse200]
    """

    return sync_detailed(
        app=app,
        upload=upload,
        client=client,
    ).parsed


async def asyncio_detailed(
    app: str,
    upload: str,
    *,
    client: AuthenticatedClient,
) -> Response[Union[Error, UploadCompleteResponse200]]:
    """Commit an upload

     Charges and commits the file; a retry answers the same file. `409 UPLOAD_INCOMPLETE` names the part
    still missing.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        upload (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, UploadCompleteResponse200]]
    """

    kwargs = _get_kwargs(
        app=app,
        upload=upload,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    upload: str,
    *,
    client: AuthenticatedClient,
) -> Optional[Union[Error, UploadCompleteResponse200]]:
    """Commit an upload

     Charges and commits the file; a retry answers the same file. `409 UPLOAD_INCOMPLETE` names the part
    still missing.

    **Who:** signed in (a session or an API key).

    Args:
        app (str):
        upload (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, UploadCompleteResponse200]
    """

    return (
        await asyncio_detailed(
            app=app,
            upload=upload,
            client=client,
        )
    ).parsed
