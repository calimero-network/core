from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.deployment_owner import DeploymentOwner
    from ..models.public_deployment_app import PublicDeploymentApp


T = TypeVar("T", bound="PublicDeployment")


@_attrs_define
class PublicDeployment:
    """`GET /deployments/{id}`, public: its name, whose it is, and what it offers.

    Attributes:
        id (str):
        host (Union[None, str]):
        name (str):
        owner (DeploymentOwner): Whose deployment it is: an account or an organization, and its name (`null`: gone).
        apps (list['PublicDeploymentApp']):
    """

    id: str
    host: Union[None, str]
    name: str
    owner: "DeploymentOwner"
    apps: list["PublicDeploymentApp"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployment_owner import DeploymentOwner
        from ..models.public_deployment_app import PublicDeploymentApp

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
        from ..models.deployment_owner import DeploymentOwner
        from ..models.public_deployment_app import PublicDeploymentApp

        d = dict(src_dict)
        id = d.pop("id")

        def _parse_host(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        host = _parse_host(d.pop("host"))

        name = d.pop("name")

        owner = DeploymentOwner.from_dict(d.pop("owner"))

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = PublicDeploymentApp.from_dict(apps_item_data)

            apps.append(apps_item)

        public_deployment = cls(
            id=id,
            host=host,
            name=name,
            owner=owner,
            apps=apps,
        )

        public_deployment.additional_properties = d
        return public_deployment

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
