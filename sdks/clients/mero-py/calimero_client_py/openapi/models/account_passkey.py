from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="AccountPasskey")


@_attrs_define
class AccountPasskey:
    """One passkey of an [`AccountView`]: its label, and when it was added and last signed in (`null`: before the node kept
    it).

        Attributes:
            id (str):
            sign_count (int):
            label (Union[None, str]):
            added_at (Union[None, int]):
            last_sign_in_at (Union[None, int]):
    """

    id: str
    sign_count: int
    label: Union[None, str]
    added_at: Union[None, int]
    last_sign_in_at: Union[None, int]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        id = self.id

        sign_count = self.sign_count

        label: Union[None, str]
        label = self.label

        added_at: Union[None, int]
        added_at = self.added_at

        last_sign_in_at: Union[None, int]
        last_sign_in_at = self.last_sign_in_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "signCount": sign_count,
                "label": label,
                "addedAt": added_at,
                "lastSignInAt": last_sign_in_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        id = d.pop("id")

        sign_count = d.pop("signCount")

        def _parse_label(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        label = _parse_label(d.pop("label"))

        def _parse_added_at(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        added_at = _parse_added_at(d.pop("addedAt"))

        def _parse_last_sign_in_at(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        last_sign_in_at = _parse_last_sign_in_at(d.pop("lastSignInAt"))

        account_passkey = cls(
            id=id,
            sign_count=sign_count,
            label=label,
            added_at=added_at,
            last_sign_in_at=last_sign_in_at,
        )

        account_passkey.additional_properties = d
        return account_passkey

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
