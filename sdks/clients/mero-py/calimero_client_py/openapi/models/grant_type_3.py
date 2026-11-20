from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.grant_type_3_kind import GrantType3Kind
from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="GrantType3")


@_attrs_define
class GrantType3:
    """
    Attributes:
        kind (GrantType3Kind):
        signed_out (Union[Unset, bool]):
    """

    kind: GrantType3Kind
    signed_out: Union[Unset, bool] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        kind = self.kind.value

        signed_out = self.signed_out

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kind": kind,
            }
        )
        if signed_out is not UNSET:
            field_dict["signedOut"] = signed_out

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        kind = GrantType3Kind(d.pop("kind"))

        signed_out = d.pop("signedOut", UNSET)

        grant_type_3 = cls(
            kind=kind,
            signed_out=signed_out,
        )

        grant_type_3.additional_properties = d
        return grant_type_3

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
