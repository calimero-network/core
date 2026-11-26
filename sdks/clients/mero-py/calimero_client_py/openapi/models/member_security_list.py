from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.member_security import MemberSecurity


T = TypeVar("T", bound="MemberSecurityList")


@_attrs_define
class MemberSecurityList:
    """`GET /orgs/{org}/members?detail=security`: each member the caller sees, with what tells whether their account is
    safe.

        Attributes:
            org_id (str):
            members (list['MemberSecurity']):
    """

    org_id: str
    members: list["MemberSecurity"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.member_security import MemberSecurity

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
        from ..models.member_security import MemberSecurity

        d = dict(src_dict)
        org_id = d.pop("orgId")

        members = []
        _members = d.pop("members")
        for members_item_data in _members:
            members_item = MemberSecurity.from_dict(members_item_data)

            members.append(members_item)

        member_security_list = cls(
            org_id=org_id,
            members=members,
        )

        member_security_list.additional_properties = d
        return member_security_list

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
