from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.deployment_offered_transfer import DeploymentOfferedTransfer


T = TypeVar("T", bound="DeploymentOffered")


@_attrs_define
class DeploymentOffered:
    """`POST /deployments/{id}/transfer`: the offer as logged (`offeredAt`: Unix seconds).

    Attributes:
        deployment (str):
        transfer (DeploymentOfferedTransfer):
    """

    deployment: str
    transfer: "DeploymentOfferedTransfer"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployment_offered_transfer import DeploymentOfferedTransfer

        deployment = self.deployment

        transfer = self.transfer.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deployment": deployment,
                "transfer": transfer,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployment_offered_transfer import DeploymentOfferedTransfer

        d = dict(src_dict)
        deployment = d.pop("deployment")

        transfer = DeploymentOfferedTransfer.from_dict(d.pop("transfer"))

        deployment_offered = cls(
            deployment=deployment,
            transfer=transfer,
        )

        deployment_offered.additional_properties = d
        return deployment_offered

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
