from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

T = TypeVar("T", bound="ConfirmRequired")


@_attrs_define
class ConfirmRequired:
    """`details` of a `412 CONFIRM_REQUIRED`: the WebAuthn challenge to sign under `rpId` with one of the passkeys
    `allowCredentials` names (credential ids; empty: any of the account's), and the `challengeId` the retry's `confirm`
    names. `apps`: deleting a team that holds apps, the apps that go with it.

        Attributes:
            challenge_id (str):
            challenge (str): base64url (RFC 4648 section 5) without padding
            rp_id (str):
            allow_credentials (list[str]):
            apps (Union[Unset, list[str]]):
    """

    challenge_id: str
    challenge: str
    rp_id: str
    allow_credentials: list[str]
    apps: Union[Unset, list[str]] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        challenge_id = self.challenge_id

        challenge = self.challenge

        rp_id = self.rp_id

        allow_credentials = self.allow_credentials

        apps: Union[Unset, list[str]] = UNSET
        if not isinstance(self.apps, Unset):
            apps = self.apps

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "challengeId": challenge_id,
                "challenge": challenge,
                "rpId": rp_id,
                "allowCredentials": allow_credentials,
            }
        )
        if apps is not UNSET:
            field_dict["apps"] = apps

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        challenge_id = d.pop("challengeId")

        challenge = d.pop("challenge")

        rp_id = d.pop("rpId")

        allow_credentials = cast(list[str], d.pop("allowCredentials"))

        apps = cast(list[str], d.pop("apps", UNSET))

        confirm_required = cls(
            challenge_id=challenge_id,
            challenge=challenge,
            rp_id=rp_id,
            allow_credentials=allow_credentials,
            apps=apps,
        )

        confirm_required.additional_properties = d
        return confirm_required

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
