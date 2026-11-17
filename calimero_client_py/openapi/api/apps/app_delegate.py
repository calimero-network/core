from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.app_delegate_body import AppDelegateBody
from ...models.app_delegate_response_200 import AppDelegateResponse200
from ...models.error import Error
from typing import cast


def _get_kwargs(
    app: str,
    target: str,
    *,
    body: AppDelegateBody,
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "put",
        "url": "/apps/{app}/delegations/{target}".format(
            app=app,
            target=target,
        ),
    }

    _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppDelegateResponse200, Error]]:
    if response.status_code == 200:
        response_200 = AppDelegateResponse200.from_dict(response.json())

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

    if response.status_code == 409:
        response_409 = Error.from_dict(response.json())

        return response_409

    if response.status_code == 412:
        response_412 = Error.from_dict(response.json())

        return response_412

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[AppDelegateResponse200, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    app: str,
    target: str,
    *,
    client: AuthenticatedClient,
    body: AppDelegateBody,
) -> Response[Union[AppDelegateResponse200, Error]]:
    """Let an app call another as you

     The methods of `target` that the release `app` runs lists for `target`'s package in its signed
    `uses` (`methods`), with your passkey over exactly those. You must reach both apps, which are in one
    organization. From then on, when `app` calls one of those methods while serving you, `target` runs
    it with you as the caller and reads `env::via()` = `app`; without the grant it runs as `app`, and
    `target` never learns who `app` was serving. A method that needs your passkey confirmation is still
    refused there. `409 NOTHING_TO_DELEGATE` when the release lists none.

    **Who:** signed in (a session or an API key). **Step-up:** answers `412 CONFIRM_REQUIRED` first;
    send the same request again with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        app (str):
        target (str):
        body (AppDelegateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppDelegateResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
        target=target,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app: str,
    target: str,
    *,
    client: AuthenticatedClient,
    body: AppDelegateBody,
) -> Optional[Union[AppDelegateResponse200, Error]]:
    """Let an app call another as you

     The methods of `target` that the release `app` runs lists for `target`'s package in its signed
    `uses` (`methods`), with your passkey over exactly those. You must reach both apps, which are in one
    organization. From then on, when `app` calls one of those methods while serving you, `target` runs
    it with you as the caller and reads `env::via()` = `app`; without the grant it runs as `app`, and
    `target` never learns who `app` was serving. A method that needs your passkey confirmation is still
    refused there. `409 NOTHING_TO_DELEGATE` when the release lists none.

    **Who:** signed in (a session or an API key). **Step-up:** answers `412 CONFIRM_REQUIRED` first;
    send the same request again with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        app (str):
        target (str):
        body (AppDelegateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppDelegateResponse200, Error]
    """

    return sync_detailed(
        app=app,
        target=target,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    app: str,
    target: str,
    *,
    client: AuthenticatedClient,
    body: AppDelegateBody,
) -> Response[Union[AppDelegateResponse200, Error]]:
    """Let an app call another as you

     The methods of `target` that the release `app` runs lists for `target`'s package in its signed
    `uses` (`methods`), with your passkey over exactly those. You must reach both apps, which are in one
    organization. From then on, when `app` calls one of those methods while serving you, `target` runs
    it with you as the caller and reads `env::via()` = `app`; without the grant it runs as `app`, and
    `target` never learns who `app` was serving. A method that needs your passkey confirmation is still
    refused there. `409 NOTHING_TO_DELEGATE` when the release lists none.

    **Who:** signed in (a session or an API key). **Step-up:** answers `412 CONFIRM_REQUIRED` first;
    send the same request again with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        app (str):
        target (str):
        body (AppDelegateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppDelegateResponse200, Error]]
    """

    kwargs = _get_kwargs(
        app=app,
        target=target,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app: str,
    target: str,
    *,
    client: AuthenticatedClient,
    body: AppDelegateBody,
) -> Optional[Union[AppDelegateResponse200, Error]]:
    """Let an app call another as you

     The methods of `target` that the release `app` runs lists for `target`'s package in its signed
    `uses` (`methods`), with your passkey over exactly those. You must reach both apps, which are in one
    organization. From then on, when `app` calls one of those methods while serving you, `target` runs
    it with you as the caller and reads `env::via()` = `app`; without the grant it runs as `app`, and
    `target` never learns who `app` was serving. A method that needs your passkey confirmation is still
    refused there. `409 NOTHING_TO_DELEGATE` when the release lists none.

    **Who:** signed in (a session or an API key). **Step-up:** answers `412 CONFIRM_REQUIRED` first;
    send the same request again with `confirm`, a passkey assertion over `details.challenge`.

    Args:
        app (str):
        target (str):
        body (AppDelegateBody):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppDelegateResponse200, Error]
    """

    return (
        await asyncio_detailed(
            app=app,
            target=target,
            client=client,
            body=body,
        )
    ).parsed
