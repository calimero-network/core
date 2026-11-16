from http import HTTPStatus
from typing import Any, Optional, Union, cast

import httpx

from ...client import AuthenticatedClient, Client
from ...types import Response, UNSET
from ... import errors

from ...models.consent_page_create_body_type_0 import ConsentPageCreateBodyType0
from ...models.consent_page_create_body_type_1 import ConsentPageCreateBodyType1
from ...models.consent_page_create_body_type_10 import ConsentPageCreateBodyType10
from ...models.consent_page_create_body_type_11 import ConsentPageCreateBodyType11
from ...models.consent_page_create_body_type_12 import ConsentPageCreateBodyType12
from ...models.consent_page_create_body_type_13 import ConsentPageCreateBodyType13
from ...models.consent_page_create_body_type_14 import ConsentPageCreateBodyType14
from ...models.consent_page_create_body_type_15 import ConsentPageCreateBodyType15
from ...models.consent_page_create_body_type_2 import ConsentPageCreateBodyType2
from ...models.consent_page_create_body_type_3 import ConsentPageCreateBodyType3
from ...models.consent_page_create_body_type_4 import ConsentPageCreateBodyType4
from ...models.consent_page_create_body_type_5 import ConsentPageCreateBodyType5
from ...models.consent_page_create_body_type_6 import ConsentPageCreateBodyType6
from ...models.consent_page_create_body_type_7 import ConsentPageCreateBodyType7
from ...models.consent_page_create_body_type_8 import ConsentPageCreateBodyType8
from ...models.consent_page_create_body_type_9 import ConsentPageCreateBodyType9
from ...models.consent_page_create_response_200 import ConsentPageCreateResponse200
from ...models.error import Error
from typing import cast
from typing import cast, Union


def _get_kwargs(
    *,
    body: Union[
        "ConsentPageCreateBodyType0",
        "ConsentPageCreateBodyType1",
        "ConsentPageCreateBodyType10",
        "ConsentPageCreateBodyType11",
        "ConsentPageCreateBodyType12",
        "ConsentPageCreateBodyType13",
        "ConsentPageCreateBodyType14",
        "ConsentPageCreateBodyType15",
        "ConsentPageCreateBodyType2",
        "ConsentPageCreateBodyType3",
        "ConsentPageCreateBodyType4",
        "ConsentPageCreateBodyType5",
        "ConsentPageCreateBodyType6",
        "ConsentPageCreateBodyType7",
        "ConsentPageCreateBodyType8",
        "ConsentPageCreateBodyType9",
    ],
) -> dict[str, Any]:
    headers: dict[str, Any] = {}

    _kwargs: dict[str, Any] = {
        "method": "post",
        "url": "/consent/requests",
    }

    _kwargs["json"]: dict[str, Any]
    if isinstance(body, ConsentPageCreateBodyType0):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType1):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType2):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType3):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType4):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType5):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType6):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType7):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType8):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType9):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType10):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType11):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType12):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType13):
        _kwargs["json"] = body.to_dict()
    elif isinstance(body, ConsentPageCreateBodyType14):
        _kwargs["json"] = body.to_dict()
    else:
        _kwargs["json"] = body.to_dict()

    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ConsentPageCreateResponse200, Error]]:
    if response.status_code == 200:
        response_200 = ConsentPageCreateResponse200.from_dict(response.json())

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

    if response.status_code == 409:
        response_409 = Error.from_dict(response.json())

        return response_409

    if response.status_code == 413:
        response_413 = Error.from_dict(response.json())

        return response_413

    if response.status_code == 421:
        response_421 = Error.from_dict(response.json())

        return response_421

    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[ConsentPageCreateResponse200, Error]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: AuthenticatedClient,
    body: Union[
        "ConsentPageCreateBodyType0",
        "ConsentPageCreateBodyType1",
        "ConsentPageCreateBodyType10",
        "ConsentPageCreateBodyType11",
        "ConsentPageCreateBodyType12",
        "ConsentPageCreateBodyType13",
        "ConsentPageCreateBodyType14",
        "ConsentPageCreateBodyType15",
        "ConsentPageCreateBodyType2",
        "ConsentPageCreateBodyType3",
        "ConsentPageCreateBodyType4",
        "ConsentPageCreateBodyType5",
        "ConsentPageCreateBodyType6",
        "ConsentPageCreateBodyType7",
        "ConsentPageCreateBodyType8",
        "ConsentPageCreateBodyType9",
    ],
) -> Response[Union[ConsentPageCreateResponse200, Error]]:
    """Ask the account's owner to approve

     From an app's frontend: a release approval, an upgrade of its instance, outbound prefixes, a new
    organization, a new invite or joining with one, leaving an organization, a member's removal, role,
    teams or capabilities, an invite's revocation, an organization's name, its deletion, defaults or
    all-apps grants, approved on the node's own consent page (`url`), which takes the passkey. What the
    approval made (`orgId`, an invite's `token`, the `org` joined, a rename's `{ orgId, name }`, a
    deletion's `apps`) is in `/consent/{request}/result`'s `outcome`. 64 KiB at most.

    **Who:** a passkey session (an API key is refused).

    Args:
        body (Union['ConsentPageCreateBodyType0', 'ConsentPageCreateBodyType1',
            'ConsentPageCreateBodyType10', 'ConsentPageCreateBodyType11',
            'ConsentPageCreateBodyType12', 'ConsentPageCreateBodyType13',
            'ConsentPageCreateBodyType14', 'ConsentPageCreateBodyType15',
            'ConsentPageCreateBodyType2', 'ConsentPageCreateBodyType3', 'ConsentPageCreateBodyType4',
            'ConsentPageCreateBodyType5', 'ConsentPageCreateBodyType6', 'ConsentPageCreateBodyType7',
            'ConsentPageCreateBodyType8', 'ConsentPageCreateBodyType9']):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ConsentPageCreateResponse200, Error]]
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
    body: Union[
        "ConsentPageCreateBodyType0",
        "ConsentPageCreateBodyType1",
        "ConsentPageCreateBodyType10",
        "ConsentPageCreateBodyType11",
        "ConsentPageCreateBodyType12",
        "ConsentPageCreateBodyType13",
        "ConsentPageCreateBodyType14",
        "ConsentPageCreateBodyType15",
        "ConsentPageCreateBodyType2",
        "ConsentPageCreateBodyType3",
        "ConsentPageCreateBodyType4",
        "ConsentPageCreateBodyType5",
        "ConsentPageCreateBodyType6",
        "ConsentPageCreateBodyType7",
        "ConsentPageCreateBodyType8",
        "ConsentPageCreateBodyType9",
    ],
) -> Optional[Union[ConsentPageCreateResponse200, Error]]:
    """Ask the account's owner to approve

     From an app's frontend: a release approval, an upgrade of its instance, outbound prefixes, a new
    organization, a new invite or joining with one, leaving an organization, a member's removal, role,
    teams or capabilities, an invite's revocation, an organization's name, its deletion, defaults or
    all-apps grants, approved on the node's own consent page (`url`), which takes the passkey. What the
    approval made (`orgId`, an invite's `token`, the `org` joined, a rename's `{ orgId, name }`, a
    deletion's `apps`) is in `/consent/{request}/result`'s `outcome`. 64 KiB at most.

    **Who:** a passkey session (an API key is refused).

    Args:
        body (Union['ConsentPageCreateBodyType0', 'ConsentPageCreateBodyType1',
            'ConsentPageCreateBodyType10', 'ConsentPageCreateBodyType11',
            'ConsentPageCreateBodyType12', 'ConsentPageCreateBodyType13',
            'ConsentPageCreateBodyType14', 'ConsentPageCreateBodyType15',
            'ConsentPageCreateBodyType2', 'ConsentPageCreateBodyType3', 'ConsentPageCreateBodyType4',
            'ConsentPageCreateBodyType5', 'ConsentPageCreateBodyType6', 'ConsentPageCreateBodyType7',
            'ConsentPageCreateBodyType8', 'ConsentPageCreateBodyType9']):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ConsentPageCreateResponse200, Error]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: AuthenticatedClient,
    body: Union[
        "ConsentPageCreateBodyType0",
        "ConsentPageCreateBodyType1",
        "ConsentPageCreateBodyType10",
        "ConsentPageCreateBodyType11",
        "ConsentPageCreateBodyType12",
        "ConsentPageCreateBodyType13",
        "ConsentPageCreateBodyType14",
        "ConsentPageCreateBodyType15",
        "ConsentPageCreateBodyType2",
        "ConsentPageCreateBodyType3",
        "ConsentPageCreateBodyType4",
        "ConsentPageCreateBodyType5",
        "ConsentPageCreateBodyType6",
        "ConsentPageCreateBodyType7",
        "ConsentPageCreateBodyType8",
        "ConsentPageCreateBodyType9",
    ],
) -> Response[Union[ConsentPageCreateResponse200, Error]]:
    """Ask the account's owner to approve

     From an app's frontend: a release approval, an upgrade of its instance, outbound prefixes, a new
    organization, a new invite or joining with one, leaving an organization, a member's removal, role,
    teams or capabilities, an invite's revocation, an organization's name, its deletion, defaults or
    all-apps grants, approved on the node's own consent page (`url`), which takes the passkey. What the
    approval made (`orgId`, an invite's `token`, the `org` joined, a rename's `{ orgId, name }`, a
    deletion's `apps`) is in `/consent/{request}/result`'s `outcome`. 64 KiB at most.

    **Who:** a passkey session (an API key is refused).

    Args:
        body (Union['ConsentPageCreateBodyType0', 'ConsentPageCreateBodyType1',
            'ConsentPageCreateBodyType10', 'ConsentPageCreateBodyType11',
            'ConsentPageCreateBodyType12', 'ConsentPageCreateBodyType13',
            'ConsentPageCreateBodyType14', 'ConsentPageCreateBodyType15',
            'ConsentPageCreateBodyType2', 'ConsentPageCreateBodyType3', 'ConsentPageCreateBodyType4',
            'ConsentPageCreateBodyType5', 'ConsentPageCreateBodyType6', 'ConsentPageCreateBodyType7',
            'ConsentPageCreateBodyType8', 'ConsentPageCreateBodyType9']):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ConsentPageCreateResponse200, Error]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: AuthenticatedClient,
    body: Union[
        "ConsentPageCreateBodyType0",
        "ConsentPageCreateBodyType1",
        "ConsentPageCreateBodyType10",
        "ConsentPageCreateBodyType11",
        "ConsentPageCreateBodyType12",
        "ConsentPageCreateBodyType13",
        "ConsentPageCreateBodyType14",
        "ConsentPageCreateBodyType15",
        "ConsentPageCreateBodyType2",
        "ConsentPageCreateBodyType3",
        "ConsentPageCreateBodyType4",
        "ConsentPageCreateBodyType5",
        "ConsentPageCreateBodyType6",
        "ConsentPageCreateBodyType7",
        "ConsentPageCreateBodyType8",
        "ConsentPageCreateBodyType9",
    ],
) -> Optional[Union[ConsentPageCreateResponse200, Error]]:
    """Ask the account's owner to approve

     From an app's frontend: a release approval, an upgrade of its instance, outbound prefixes, a new
    organization, a new invite or joining with one, leaving an organization, a member's removal, role,
    teams or capabilities, an invite's revocation, an organization's name, its deletion, defaults or
    all-apps grants, approved on the node's own consent page (`url`), which takes the passkey. What the
    approval made (`orgId`, an invite's `token`, the `org` joined, a rename's `{ orgId, name }`, a
    deletion's `apps`) is in `/consent/{request}/result`'s `outcome`. 64 KiB at most.

    **Who:** a passkey session (an API key is refused).

    Args:
        body (Union['ConsentPageCreateBodyType0', 'ConsentPageCreateBodyType1',
            'ConsentPageCreateBodyType10', 'ConsentPageCreateBodyType11',
            'ConsentPageCreateBodyType12', 'ConsentPageCreateBodyType13',
            'ConsentPageCreateBodyType14', 'ConsentPageCreateBodyType15',
            'ConsentPageCreateBodyType2', 'ConsentPageCreateBodyType3', 'ConsentPageCreateBodyType4',
            'ConsentPageCreateBodyType5', 'ConsentPageCreateBodyType6', 'ConsentPageCreateBodyType7',
            'ConsentPageCreateBodyType8', 'ConsentPageCreateBodyType9']):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ConsentPageCreateResponse200, Error]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
