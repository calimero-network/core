from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.org_role import OrgRole
from ..types import UNSET, Unset
from typing import Union

T = TypeVar("T", bound="MeResponse200OrgsItem")


@_attrs_define
class MeResponse200OrgsItem:
    """
    Attributes:
        org_id (str):
        name (str):
        role (OrgRole): A member's role in an organization.
        personal (Union[Unset, bool]):
    """

    org_id: str
    name: str
    role: OrgRole
    personal: Union[Unset, bool] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        org_id = self.org_id

        name = self.name

        role = self.role.value

        personal = self.personal

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "orgId": org_id,
                "name": name,
                "role": role,
            }
        )
        if personal is not UNSET:
            field_dict["personal"] = personal

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        org_id = d.pop("orgId")

        name = d.pop("name")

        role = OrgRole(d.pop("role"))

        personal = d.pop("personal", UNSET)

        me_response_200_orgs_item = cls(
            org_id=org_id,
            name=name,
            role=role,
            personal=personal,
        )

        me_response_200_orgs_item.additional_properties = d
        return me_response_200_orgs_item

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
