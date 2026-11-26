from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.unit_member import UnitMember


T = TypeVar("T", bound="CapabilityCardMembersType0")


@_attrs_define
class CapabilityCardMembersType0:
    """ """

    additional_properties: dict[str, "UnitMember"] = _attrs_field(
        init=False, factory=dict
    )

    def to_dict(self) -> dict[str, Any]:
        from ..models.unit_member import UnitMember

        field_dict: dict[str, Any] = {}
        for prop_name, prop in self.additional_properties.items():
            field_dict[prop_name] = prop.to_dict()

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.unit_member import UnitMember

        d = dict(src_dict)
        capability_card_members_type_0 = cls()

        additional_properties = {}
        for prop_name, prop_dict in d.items():
            additional_property = UnitMember.from_dict(prop_dict)

            additional_properties[prop_name] = additional_property

        capability_card_members_type_0.additional_properties = additional_properties
        return capability_card_members_type_0

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> "UnitMember":
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: "UnitMember") -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
