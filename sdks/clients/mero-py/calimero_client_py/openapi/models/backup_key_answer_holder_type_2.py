from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.backup_key_answer_holder_type_2_kind import BackupKeyAnswerHolderType2Kind

T = TypeVar("T", bound="BackupKeyAnswerHolderType2")


@_attrs_define
class BackupKeyAnswerHolderType2:
    """
    Attributes:
        kind (BackupKeyAnswerHolderType2Kind):
    """

    kind: BackupKeyAnswerHolderType2Kind
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = BackupKeyAnswerHolderType2Kind(d.pop("kind"))

        backup_key_answer_holder_type_2 = cls(
            kind=kind,
        )

        backup_key_answer_holder_type_2.additional_properties = d
        return backup_key_answer_holder_type_2

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
