from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.deployer_list_deployers import DeployerListDeployers
from ..models.deployer_list_deployments import DeployerListDeployments
from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.deployer_type_0 import DeployerType0
    from ..models.deployer_grant_view import DeployerGrantView
    from ..models.deployer_type_1 import DeployerType1


T = TypeVar("T", bound="DeployerList")


@_attrs_define
class DeployerList:
    """`GET /deployers`: the cluster's deployment rules, its owner and every grant.

    Attributes:
        deployments (DeployerListDeployments):
        deployers (DeployerListDeployers):
        owner (Union['DeployerType0', 'DeployerType1', None]):
        grants (list['DeployerGrantView']):
    """

    deployments: DeployerListDeployments
    deployers: DeployerListDeployers
    owner: Union["DeployerType0", "DeployerType1", None]
    grants: list["DeployerGrantView"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_grant_view import DeployerGrantView
        from ..models.deployer_type_1 import DeployerType1

        deployments = self.deployments.value

        deployers = self.deployers.value

        owner: Union[None, dict[str, Any]]
        if isinstance(self.owner, DeployerType0):
            owner = self.owner.to_dict()
        elif isinstance(self.owner, DeployerType1):
            owner = self.owner.to_dict()
        else:
            owner = self.owner

        grants = []
        for grants_item_data in self.grants:
            grants_item = grants_item_data.to_dict()
            grants.append(grants_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deployments": deployments,
                "deployers": deployers,
                "owner": owner,
                "grants": grants,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployer_type_0 import DeployerType0
        from ..models.deployer_grant_view import DeployerGrantView
        from ..models.deployer_type_1 import DeployerType1

        d = dict(src_dict)
        deployments = DeployerListDeployments(d.pop("deployments"))

        deployers = DeployerListDeployers(d.pop("deployers"))

        def _parse_owner(data: object) -> Union["DeployerType0", "DeployerType1", None]:
            if data is None:
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_deployer_type_0 = DeployerType0.from_dict(data)

                return componentsschemas_deployer_type_0
            except:  # noqa: E722
                pass
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_deployer_type_1 = DeployerType1.from_dict(data)

                return componentsschemas_deployer_type_1
            except:  # noqa: E722
                pass
            return cast(Union["DeployerType0", "DeployerType1", None], data)

        owner = _parse_owner(d.pop("owner"))

        grants = []
        _grants = d.pop("grants")
        for grants_item_data in _grants:
            grants_item = DeployerGrantView.from_dict(grants_item_data)

            grants.append(grants_item)

        deployer_list = cls(
            deployments=deployments,
            deployers=deployers,
            owner=owner,
            grants=grants,
        )

        deployer_list.additional_properties = d
        return deployer_list

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
