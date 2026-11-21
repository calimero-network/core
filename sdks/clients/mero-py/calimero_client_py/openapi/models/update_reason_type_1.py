from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.update_reason_type_1_code import UpdateReasonType1Code
from typing import cast

T = TypeVar("T", bound="UpdateReasonType1")


@_attrs_define
class UpdateReasonType1:
    """
    Attributes:
        code (UpdateReasonType1Code):
        destinations (list[str]):
    """

    code: UpdateReasonType1Code
    destinations: list[str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        code = self.code.value

        destinations = self.destinations

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "code": code,
                "destinations": destinations,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        code = UpdateReasonType1Code(d.pop("code"))

        destinations = cast(list[str], d.pop("destinations"))

        update_reason_type_1 = cls(
            code=code,
            destinations=destinations,
        )

        update_reason_type_1.additional_properties = d
        return update_reason_type_1

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
