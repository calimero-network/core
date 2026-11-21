from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast, Union

T = TypeVar("T", bound="UndoPartLostWritesType0")


@_attrs_define
class UndoPartLostWritesType0:
    """
    Attributes:
        entries_since (Union[None, int]):
        last_write_at (Union[None, int]):
        any_ (Union[None, bool]):
    """

    entries_since: Union[None, int]
    last_write_at: Union[None, int]
    any_: Union[None, bool]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        entries_since: Union[None, int]
        entries_since = self.entries_since

        last_write_at: Union[None, int]
        last_write_at = self.last_write_at

        any_: Union[None, bool]
        any_ = self.any_

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "entriesSince": entries_since,
                "lastWriteAt": last_write_at,
                "any": any_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)

        def _parse_entries_since(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        entries_since = _parse_entries_since(d.pop("entriesSince"))

        def _parse_last_write_at(data: object) -> Union[None, int]:
            if data is None:
                return data
            return cast(Union[None, int], data)

        last_write_at = _parse_last_write_at(d.pop("lastWriteAt"))

        def _parse_any_(data: object) -> Union[None, bool]:
            if data is None:
                return data
            return cast(Union[None, bool], data)

        any_ = _parse_any_(d.pop("any"))

        undo_part_lost_writes_type_0 = cls(
            entries_since=entries_since,
            last_write_at=last_write_at,
            any_=any_,
        )

        undo_part_lost_writes_type_0.additional_properties = d
        return undo_part_lost_writes_type_0

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
