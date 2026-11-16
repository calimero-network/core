from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="BackupKeyAnswerKeyType0")


@_attrs_define
class BackupKeyAnswerKeyType0:
    """
    Attributes:
        public_key (str):
        set_at (int):
        set_by (str):
    """

    public_key: str
    set_at: int
    set_by: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        public_key = self.public_key

        set_at = self.set_at

        set_by = self.set_by

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "publicKey": public_key,
                "setAt": set_at,
                "setBy": set_by,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        public_key = d.pop("publicKey")

        set_at = d.pop("setAt")

        set_by = d.pop("setBy")

        backup_key_answer_key_type_0 = cls(
            public_key=public_key,
            set_at=set_at,
            set_by=set_by,
        )

        backup_key_answer_key_type_0.additional_properties = d
        return backup_key_answer_key_type_0

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
