from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="PasskeyAddBeginResponse200")


@_attrs_define
class PasskeyAddBeginResponse200:
    """
    Attributes:
        challenge_id (str):
        options (Any): Free-form JSON: its shape is not this API's (see the description around it for whose it is).
    """

    challenge_id: str
    options: Any
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        challenge_id = self.challenge_id

        options = self.options

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "challengeId": challenge_id,
                "options": options,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        challenge_id = d.pop("challengeId")

        options = d.pop("options")

        passkey_add_begin_response_200 = cls(
            challenge_id=challenge_id,
            options=options,
        )

        passkey_add_begin_response_200.additional_properties = d
        return passkey_add_begin_response_200

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
