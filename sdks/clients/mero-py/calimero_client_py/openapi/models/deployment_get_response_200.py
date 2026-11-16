from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.deployment_get_response_200_apps_item import (
        DeploymentGetResponse200AppsItem,
    )
    from ..models.deployment_get_response_200_owner import DeploymentGetResponse200Owner


T = TypeVar("T", bound="DeploymentGetResponse200")


@_attrs_define
class DeploymentGetResponse200:
    """
    Attributes:
        id (str):
        host (Union[None, str]):
        name (str):
        owner (DeploymentGetResponse200Owner):
        apps (list['DeploymentGetResponse200AppsItem']):
    """

    id: str
    host: Union[None, str]
    name: str
    owner: "DeploymentGetResponse200Owner"
    apps: list["DeploymentGetResponse200AppsItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployment_get_response_200_apps_item import (
            DeploymentGetResponse200AppsItem,
        )
        from ..models.deployment_get_response_200_owner import (
            DeploymentGetResponse200Owner,
        )

        id = self.id

        host: Union[None, str]
        host = self.host

        name = self.name

        owner = self.owner.to_dict()

        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "host": host,
                "name": name,
                "owner": owner,
                "apps": apps,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployment_get_response_200_apps_item import (
            DeploymentGetResponse200AppsItem,
        )
        from ..models.deployment_get_response_200_owner import (
            DeploymentGetResponse200Owner,
        )

        d = dict(src_dict)
        id = d.pop("id")

        def _parse_host(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        host = _parse_host(d.pop("host"))

        name = d.pop("name")

        owner = DeploymentGetResponse200Owner.from_dict(d.pop("owner"))

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = DeploymentGetResponse200AppsItem.from_dict(apps_item_data)

            apps.append(apps_item)

        deployment_get_response_200 = cls(
            id=id,
            host=host,
            name=name,
            owner=owner,
            apps=apps,
        )

        deployment_get_response_200.additional_properties = d
        return deployment_get_response_200

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
