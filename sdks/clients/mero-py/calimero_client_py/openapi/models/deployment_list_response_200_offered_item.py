from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.deployment_list_response_200_offered_item_owner import (
    DeploymentListResponse200OfferedItemOwner,
)
from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.deployer_type_0 import DeployerType0
    from ..models.deployment_list_response_200_offered_item_apps_item import (
        DeploymentListResponse200OfferedItemAppsItem,
    )
    from ..models.deployer_type_1 import DeployerType1


T = TypeVar("T", bound="DeploymentListResponse200OfferedItem")


@_attrs_define
class DeploymentListResponse200OfferedItem:
    """
    Attributes:
        id (str):
        name (str):
        owner (DeploymentListResponse200OfferedItemOwner):
        owner_name (Union[None, str]):
        apps (list['DeploymentListResponse200OfferedItemAppsItem']):
        to (Union['DeployerType0', 'DeployerType1']): An account or an organization.
        offered_at (str):
        expires_at (str):
    """

    id: str
    name: str
    owner: DeploymentListResponse200OfferedItemOwner
    owner_name: Union[None, str]
    apps: list["DeploymentListResponse200OfferedItemAppsItem"]
    to: Union["DeployerType0", "DeployerType1"]
    offered_at: str
    expires_at: str
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployment_list_response_200_offered_item_apps_item import (
            DeploymentListResponse200OfferedItemAppsItem,
        )
        from ..models.deployer_type_1 import DeployerType1

        id = self.id

        name = self.name

        owner = self.owner.value

        owner_name: Union[None, str]
        owner_name = self.owner_name

        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        to: dict[str, Any]
        if isinstance(self.to, DeployerType0):
            to = self.to.to_dict()
        else:
            to = self.to.to_dict()

        offered_at = self.offered_at

        expires_at = self.expires_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "name": name,
                "owner": owner,
                "ownerName": owner_name,
                "apps": apps,
                "to": to,
                "offeredAt": offered_at,
                "expiresAt": expires_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployment_list_response_200_offered_item_apps_item import (
            DeploymentListResponse200OfferedItemAppsItem,
        )
        from ..models.deployer_type_1 import DeployerType1

        d = dict(src_dict)
        id = d.pop("id")

        name = d.pop("name")

        owner = DeploymentListResponse200OfferedItemOwner(d.pop("owner"))

        def _parse_owner_name(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        owner_name = _parse_owner_name(d.pop("ownerName"))

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = DeploymentListResponse200OfferedItemAppsItem.from_dict(
                apps_item_data
            )

            apps.append(apps_item)

        def _parse_to(data: object) -> Union["DeployerType0", "DeployerType1"]:
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_deployer_type_0 = DeployerType0.from_dict(data)

                return componentsschemas_deployer_type_0
            except:  # noqa: E722
                pass
            if not isinstance(data, dict):
                raise TypeError()
            componentsschemas_deployer_type_1 = DeployerType1.from_dict(data)

            return componentsschemas_deployer_type_1

        to = _parse_to(d.pop("to"))

        offered_at = d.pop("offeredAt")

        expires_at = d.pop("expiresAt")

        deployment_list_response_200_offered_item = cls(
            id=id,
            name=name,
            owner=owner,
            owner_name=owner_name,
            apps=apps,
            to=to,
            offered_at=offered_at,
            expires_at=expires_at,
        )

        deployment_list_response_200_offered_item.additional_properties = d
        return deployment_list_response_200_offered_item

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
