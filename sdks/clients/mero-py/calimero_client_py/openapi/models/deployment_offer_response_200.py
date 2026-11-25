from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.deployment_offer_response_200_offer import (
        DeploymentOfferResponse200Offer,
    )


T = TypeVar("T", bound="DeploymentOfferResponse200")


@_attrs_define
class DeploymentOfferResponse200:
    """
    Attributes:
        deployment (str):
        app (str):
        offer (DeploymentOfferResponse200Offer):
    """

    deployment: str
    app: str
    offer: "DeploymentOfferResponse200Offer"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployment_offer_response_200_offer import (
            DeploymentOfferResponse200Offer,
        )

        deployment = self.deployment

        app = self.app

        offer = self.offer.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deployment": deployment,
                "app": app,
                "offer": offer,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployment_offer_response_200_offer import (
            DeploymentOfferResponse200Offer,
        )

        d = dict(src_dict)
        deployment = d.pop("deployment")

        app = d.pop("app")

        offer = DeploymentOfferResponse200Offer.from_dict(d.pop("offer"))

        deployment_offer_response_200 = cls(
            deployment=deployment,
            app=app,
            offer=offer,
        )

        deployment_offer_response_200.additional_properties = d
        return deployment_offer_response_200

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
