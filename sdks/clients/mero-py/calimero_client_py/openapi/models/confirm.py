from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.assertion import Assertion


T = TypeVar("T", bound="Confirm")


@_attrs_define
class Confirm:
    """A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same request sent again with it approves
    exactly that request.

        Attributes:
            challenge_id (str):
            assertion (Assertion): The browser's answer to `navigator.credentials.get`, each field base64url.
    """

    challenge_id: str
    assertion: "Assertion"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.assertion import Assertion

        challenge_id = self.challenge_id

        assertion = self.assertion.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "challengeId": challenge_id,
                "assertion": assertion,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.assertion import Assertion

        d = dict(src_dict)
        challenge_id = d.pop("challengeId")

        assertion = Assertion.from_dict(d.pop("assertion"))

        confirm = cls(
            challenge_id=challenge_id,
            assertion=assertion,
        )

        confirm.additional_properties = d
        return confirm

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
