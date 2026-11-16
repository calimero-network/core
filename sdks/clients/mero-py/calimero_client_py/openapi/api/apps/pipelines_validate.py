from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.error import Error
from ...models.pipelines_validate_body import PipelinesValidateBody
from ...models.pipelines_validate_response_200 import PipelinesValidateResponse200
from typing import cast


def _get_kwargs(
    *,
    body: PipelinesValidateBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/pipelines/validate",
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Error, PipelinesValidateResponse200]]:
    if response.status_code == 200:
        response_200 = PipelinesValidateResponse200.from_dict(response.json())

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
) -> Response[Union[Error, PipelinesValidateResponse200]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    body: PipelinesValidateBody,
) -> Response[Union[Error, PipelinesValidateResponse200]]:
    """Validate a pipeline

     Every problem of `source`; with `package` and `version` (both, a release the caller sees) also
    against that release's methods, modules and the cluster's pools. Writes nothing.

    **Who:** signed in (a session or an API key).

    Args:
        body (PipelinesValidateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PipelinesValidateResponse200]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: AuthenticatedClient,
    body: PipelinesValidateBody,
) -> Optional[Union[Error, PipelinesValidateResponse200]]:
    """Validate a pipeline

     Every problem of `source`; with `package` and `version` (both, a release the caller sees) also
    against that release's methods, modules and the cluster's pools. Writes nothing.

    **Who:** signed in (a session or an API key).

    Args:
        body (PipelinesValidateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PipelinesValidateResponse200]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    body: PipelinesValidateBody,
) -> Response[Union[Error, PipelinesValidateResponse200]]:
    """Validate a pipeline

     Every problem of `source`; with `package` and `version` (both, a release the caller sees) also
    against that release's methods, modules and the cluster's pools. Writes nothing.

    **Who:** signed in (a session or an API key).

    Args:
        body (PipelinesValidateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Error, PipelinesValidateResponse200]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    body: PipelinesValidateBody,
) -> Optional[Union[Error, PipelinesValidateResponse200]]:
    """Validate a pipeline

     Every problem of `source`; with `package` and `version` (both, a release the caller sees) also
    against that release's methods, modules and the cluster's pools. Writes nothing.

    **Who:** signed in (a session or an API key).

    Args:
        body (PipelinesValidateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Error, PipelinesValidateResponse200]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
