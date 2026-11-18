from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.deployer_type_0 import DeployerType0
    from ..models.deployer_type_1 import DeployerType1
    from ..models.deployment_record_apps import DeploymentRecordApps


T = TypeVar("T", bound="DeploymentRecord")


@_attrs_define
class DeploymentRecord:
    """A deployment as the directory records it: the release it offers by app name (`addedAt` Unix seconds), who created it
    and when (Unix seconds).

        Attributes:
            id (str):
            name (str):
            owner (Union['DeployerType0', 'DeployerType1']): An account or an organization.
            apps (DeploymentRecordApps):
            created_by (str):
            created_at (int):
    """

    id: str
    name: str
    owner: Union["DeployerType0", "DeployerType1"]
    apps: "DeploymentRecordApps"
    created_by: str
    created_at: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_type_1 import DeployerType1
        from ..models.deployment_record_apps import DeploymentRecordApps

        id = self.id

        name = self.name

        owner: dict[str, Any]
        if isinstance(self.owner, DeployerType0):
            owner = self.owner.to_dict()
        else:
            owner = self.owner.to_dict()

        apps = self.apps.to_dict()

        created_by = self.created_by

        created_at = self.created_at

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "name": name,
                "owner": owner,
                "apps": apps,
                "createdBy": created_by,
                "createdAt": created_at,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_type_1 import DeployerType1
        from ..models.deployment_record_apps import DeploymentRecordApps

        d = dict(src_dict)
        id = d.pop("id")

        name = d.pop("name")

        def _parse_owner(data: object) -> Union["DeployerType0", "DeployerType1"]:
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

        owner = _parse_owner(d.pop("owner"))

        apps = DeploymentRecordApps.from_dict(d.pop("apps"))

        created_by = d.pop("createdBy")

        created_at = d.pop("createdAt")

        deployment_record = cls(
            id=id,
            name=name,
            owner=owner,
            apps=apps,
            created_by=created_by,
            created_at=created_at,
        )

        deployment_record.additional_properties = d
        return deployment_record

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
