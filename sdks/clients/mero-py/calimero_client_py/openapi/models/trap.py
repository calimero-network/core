from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.trap_kind import TrapKind

T = TypeVar("T", bound="Trap")


@_attrs_define
class Trap:
    """`details` of a `422 TRAP`: the log index of the run, how it trapped, and the engine's account of it.

    Attributes:
        index (int):
        kind (TrapKind):
        detail (str):
    """

    index: int
    kind: TrapKind
    detail: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        index = self.index

        kind = self.kind.value

        detail = self.detail

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "index": index,
                "kind": kind,
                "detail": detail,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        index = d.pop("index")

        kind = TrapKind(d.pop("kind"))

        detail = d.pop("detail")

        trap = cls(
            index=index,
            kind=kind,
            detail=detail,
        )

        trap.additional_properties = d
        return trap

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
