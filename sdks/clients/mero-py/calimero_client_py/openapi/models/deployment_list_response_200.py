from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.deployment import Deployment
    from ..models.deployment_list_response_200_offered_item import (
        DeploymentListResponse200OfferedItem,
    )


T = TypeVar("T", bound="DeploymentListResponse200")


@_attrs_define
class DeploymentListResponse200:
    """
    Attributes:
        deployments (list['Deployment']):
        offered (list['DeploymentListResponse200OfferedItem']):
    """

    deployments: list["Deployment"]
    offered: list["DeploymentListResponse200OfferedItem"]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployment import Deployment
        from ..models.deployment_list_response_200_offered_item import (
            DeploymentListResponse200OfferedItem,
        )

        deployments = []
        for deployments_item_data in self.deployments:
            deployments_item = deployments_item_data.to_dict()
            deployments.append(deployments_item)

        offered = []
        for offered_item_data in self.offered:
            offered_item = offered_item_data.to_dict()
            offered.append(offered_item)

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deployments": deployments,
                "offered": offered,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployment import Deployment
        from ..models.deployment_list_response_200_offered_item import (
            DeploymentListResponse200OfferedItem,
        )

        d = dict(src_dict)
        deployments = []
        _deployments = d.pop("deployments")
        for deployments_item_data in _deployments:
            deployments_item = Deployment.from_dict(deployments_item_data)

            deployments.append(deployments_item)

        offered = []
        _offered = d.pop("offered")
        for offered_item_data in _offered:
            offered_item = DeploymentListResponse200OfferedItem.from_dict(
                offered_item_data
            )

            offered.append(offered_item)

        deployment_list_response_200 = cls(
            deployments=deployments,
            offered=offered,
        )

        deployment_list_response_200.additional_properties = d
        return deployment_list_response_200

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
