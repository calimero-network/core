from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.deployment_orgs_response_200_orgs_item import (
        DeploymentOrgsResponse200OrgsItem,
    )


T = TypeVar("T", bound="DeploymentOrgsResponse200")


@_attrs_define
class DeploymentOrgsResponse200:
    """
    Attributes:
        deployment (str):
        orgs (list['DeploymentOrgsResponse200OrgsItem']):
        outside_orgs (int):
    """

    deployment: str
    orgs: list["DeploymentOrgsResponse200OrgsItem"]
    outside_orgs: int
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployment_orgs_response_200_orgs_item import (
            DeploymentOrgsResponse200OrgsItem,
        )

        deployment = self.deployment

        orgs = []
        for orgs_item_data in self.orgs:
            orgs_item = orgs_item_data.to_dict()
            orgs.append(orgs_item)

        outside_orgs = self.outside_orgs

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deployment": deployment,
                "orgs": orgs,
                "outsideOrgs": outside_orgs,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployment_orgs_response_200_orgs_item import (
            DeploymentOrgsResponse200OrgsItem,
        )

        d = dict(src_dict)
        deployment = d.pop("deployment")

        orgs = []
        _orgs = d.pop("orgs")
        for orgs_item_data in _orgs:
            orgs_item = DeploymentOrgsResponse200OrgsItem.from_dict(orgs_item_data)

            orgs.append(orgs_item)

        outside_orgs = d.pop("outsideOrgs")

        deployment_orgs_response_200 = cls(
            deployment=deployment,
            orgs=orgs,
            outside_orgs=outside_orgs,
        )

        deployment_orgs_response_200.additional_properties = d
        return deployment_orgs_response_200

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
