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


T = TypeVar("T", bound="DeploymentAccepted")


@_attrs_define
class DeploymentAccepted:
    """`POST /deployments/{id}/transfer/accept`: its new owner, the owner it came from, and the secret names whose values
    were dropped.

        Attributes:
            deployment (str):
            owner (Union['DeployerType0', 'DeployerType1']): An account or an organization.
            from_ (Union['DeployerType0', 'DeployerType1']): An account or an organization.
            secrets_dropped (list[str]):
    """

    deployment: str
    owner: Union["DeployerType0", "DeployerType1"]
    from_: Union["DeployerType0", "DeployerType1"]
    secrets_dropped: list[str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_type_1 import DeployerType1

        deployment = self.deployment

        owner: dict[str, Any]
        if isinstance(self.owner, DeployerType0):
            owner = self.owner.to_dict()
        else:
            owner = self.owner.to_dict()

        from_: dict[str, Any]
        if isinstance(self.from_, DeployerType0):
            from_ = self.from_.to_dict()
        else:
            from_ = self.from_.to_dict()

        secrets_dropped = self.secrets_dropped

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deployment": deployment,
                "owner": owner,
                "from": from_,
                "secretsDropped": secrets_dropped,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_type_1 import DeployerType1

        d = dict(src_dict)
        deployment = d.pop("deployment")

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

        def _parse_from_(data: object) -> Union["DeployerType0", "DeployerType1"]:
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

        from_ = _parse_from_(d.pop("from"))

        secrets_dropped = cast(list[str], d.pop("secretsDropped"))

        deployment_accepted = cls(
            deployment=deployment,
            owner=owner,
            from_=from_,
            secrets_dropped=secrets_dropped,
        )

        deployment_accepted.additional_properties = d
        return deployment_accepted

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
