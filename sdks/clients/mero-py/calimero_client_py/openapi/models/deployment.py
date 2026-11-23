from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.deployer_type_1 import DeployerType1
    from ..models.deployer_type_0 import DeployerType0
    from ..models.deployment_transfer_type_0 import DeploymentTransferType0
    from ..models.deployment_transferred_type_0 import DeploymentTransferredType0
    from ..models.deployment_apps_item import DeploymentAppsItem


T = TypeVar("T", bound="Deployment")


@_attrs_define
class Deployment:
    """A deployment as its managers list it. `host` is its frontend host, `null` on a node with no apps domain; times are
    RFC 3339. `transfer`: the offer of it to another owner waiting for acceptance (`expiresAt`: 7 days after
    `offeredAt`), else `null`; `transferred`: whom it came from at its last hand-over, else `null`.

        Attributes:
            id (str):
            host (Union[None, str]):
            name (str):
            owner (Union['DeployerType0', 'DeployerType1']): An account or an organization.
            apps (list['DeploymentAppsItem']):
            created_by (str):
            created_at (str):
            transfer (Union['DeploymentTransferType0', None, Unset]):
            transferred (Union['DeploymentTransferredType0', None, Unset]):
    """

    id: str
    host: Union[None, str]
    name: str
    owner: Union["DeployerType0", "DeployerType1"]
    apps: list["DeploymentAppsItem"]
    created_by: str
    created_at: str
    transfer: Union["DeploymentTransferType0", None, Unset] = UNSET
    transferred: Union["DeploymentTransferredType0", None, Unset] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployer_type_1 import DeployerType1
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployment_transfer_type_0 import DeploymentTransferType0
        from ..models.deployment_transferred_type_0 import DeploymentTransferredType0
        from ..models.deployment_apps_item import DeploymentAppsItem

        id = self.id

        host: Union[None, str]
        host = self.host

        name = self.name

        owner: dict[str, Any]
        if isinstance(self.owner, DeployerType0):
            owner = self.owner.to_dict()
        else:
            owner = self.owner.to_dict()

        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        created_by = self.created_by

        created_at = self.created_at

        transfer: Union[None, Unset, dict[str, Any]]
        if isinstance(self.transfer, Unset):
            transfer = UNSET
        elif isinstance(self.transfer, DeploymentTransferType0):
            transfer = self.transfer.to_dict()
        else:
            transfer = self.transfer

        transferred: Union[None, Unset, dict[str, Any]]
        if isinstance(self.transferred, Unset):
            transferred = UNSET
        elif isinstance(self.transferred, DeploymentTransferredType0):
            transferred = self.transferred.to_dict()
        else:
            transferred = self.transferred

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "host": host,
                "name": name,
                "owner": owner,
                "apps": apps,
                "createdBy": created_by,
                "createdAt": created_at,
            }
        )
        if transfer is not UNSET:
            field_dict["transfer"] = transfer
        if transferred is not UNSET:
            field_dict["transferred"] = transferred

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployer_type_1 import DeployerType1
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployment_transfer_type_0 import DeploymentTransferType0
        from ..models.deployment_transferred_type_0 import DeploymentTransferredType0
        from ..models.deployment_apps_item import DeploymentAppsItem

        d = dict(src_dict)
        id = d.pop("id")

        def _parse_host(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        host = _parse_host(d.pop("host"))

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

        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = DeploymentAppsItem.from_dict(apps_item_data)

            apps.append(apps_item)

        created_by = d.pop("createdBy")

        created_at = d.pop("createdAt")

        def _parse_transfer(
            data: object,
        ) -> Union["DeploymentTransferType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                transfer_type_0 = DeploymentTransferType0.from_dict(data)

                return transfer_type_0
            except:  # noqa: E722
                pass
            return cast(Union["DeploymentTransferType0", None, Unset], data)

        transfer = _parse_transfer(d.pop("transfer", UNSET))

        def _parse_transferred(
            data: object,
        ) -> Union["DeploymentTransferredType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                transferred_type_0 = DeploymentTransferredType0.from_dict(data)

                return transferred_type_0
            except:  # noqa: E722
                pass
            return cast(Union["DeploymentTransferredType0", None, Unset], data)

        transferred = _parse_transferred(d.pop("transferred", UNSET))

        deployment = cls(
            id=id,
            host=host,
            name=name,
            owner=owner,
            apps=apps,
            created_by=created_by,
            created_at=created_at,
            transfer=transfer,
            transferred=transferred,
        )

        deployment.additional_properties = d
        return deployment

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
