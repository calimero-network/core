from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.deployment_orgs_response_200_orgs_item_apps_item import (
        DeploymentOrgsResponse200OrgsItemAppsItem,
    )


T = TypeVar("T", bound="DeploymentOrgsResponse200OrgsItem")


@_attrs_define
class DeploymentOrgsResponse200OrgsItem:
    """
    Attributes:
        org_id (str):
        name (Union[None, str]):
        member_count (int):
        active_members_30_d (int):
        instances (int):
        apps (list['DeploymentOrgsResponse200OrgsItemAppsItem']):
        first_created_at (str):
    """

    org_id: str
    name: Union[None, str]
    member_count: int
    active_members_30_d: int
    instances: int
    apps: list["DeploymentOrgsResponse200OrgsItemAppsItem"]
    first_created_at: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployment_orgs_response_200_orgs_item_apps_item import (
            DeploymentOrgsResponse200OrgsItemAppsItem,
        )

        org_id = self.org_id

        name: Union[None, str]
        name = self.name

        member_count = self.member_count

        active_members_30_d = self.active_members_30_d

        instances = self.instances

        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        first_created_at = self.first_created_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "orgId": org_id,
                "name": name,
                "memberCount": member_count,
                "activeMembers30d": active_members_30_d,
                "instances": instances,
                "apps": apps,
                "firstCreatedAt": first_created_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployment_orgs_response_200_orgs_item_apps_item import (
            DeploymentOrgsResponse200OrgsItemAppsItem,
        )

        d = dict(src_dict)
        org_id = d.pop("orgId")

        def _parse_name(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        name = _parse_name(d.pop("name"))

        member_count = d.pop("memberCount")

        active_members_30_d = d.pop("activeMembers30d")

        instances = d.pop("instances")

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = DeploymentOrgsResponse200OrgsItemAppsItem.from_dict(
                apps_item_data
            )

            apps.append(apps_item)

        first_created_at = d.pop("firstCreatedAt")

        deployment_orgs_response_200_orgs_item = cls(
            org_id=org_id,
            name=name,
            member_count=member_count,
            active_members_30_d=active_members_30_d,
            instances=instances,
            apps=apps,
            first_created_at=first_created_at,
        )

        deployment_orgs_response_200_orgs_item.additional_properties = d
        return deployment_orgs_response_200_orgs_item

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
