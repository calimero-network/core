from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.backup_key_answer_holder_kind import BackupKeyAnswerHolderKind
from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="BackupKeyAnswerHolder")


@_attrs_define
class BackupKeyAnswerHolder:
    """
    Attributes:
        kind (BackupKeyAnswerHolderKind):
        id (Union[Unset, str]):
    """

    kind: BackupKeyAnswerHolderKind
    id: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        id = self.id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
            }
        )
        if id is not UNSET:
            field_dict["id"] = id

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = BackupKeyAnswerHolderKind(d.pop("kind"))

        id = d.pop("id", UNSET)

        backup_key_answer_holder = cls(
            kind=kind,
            id=id,
        )

        backup_key_answer_holder.additional_properties = d
        return backup_key_answer_holder

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
