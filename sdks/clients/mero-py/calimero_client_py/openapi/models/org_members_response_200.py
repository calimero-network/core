from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.org_members_response_200_members_item import (
        OrgMembersResponse200MembersItem,
    )


T = TypeVar("T", bound="OrgMembersResponse200")


@_attrs_define
class OrgMembersResponse200:
    """
    Attributes:
        org_id (str):
        members (list['OrgMembersResponse200MembersItem']):
    """

    org_id: str
    members: list["OrgMembersResponse200MembersItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.org_members_response_200_members_item import (
            OrgMembersResponse200MembersItem,
        )

        org_id = self.org_id

        members = []
        for members_item_data in self.members:
            members_item = members_item_data.to_dict()
            members.append(members_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "orgId": org_id,
                "members": members,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.org_members_response_200_members_item import (
            OrgMembersResponse200MembersItem,
        )

        d = dict(src_dict)
        org_id = d.pop("orgId")

        members = []
        _members = d.pop("members")
        for members_item_data in _members:
            members_item = OrgMembersResponse200MembersItem.from_dict(members_item_data)

            members.append(members_item)

        org_members_response_200 = cls(
            org_id=org_id,
            members=members,
        )

        org_members_response_200.additional_properties = d
        return org_members_response_200

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
