from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.offer import Offer


T = TypeVar("T", bound="AppOffered")


@_attrs_define
class AppOffered:
    """What `OfferApp` answers: the offer now in force.

    Attributes:
        deployment (str):
        app (str):
        offer (Offer): An app a deployment offers: the release new instances run, who offered it and when (Unix
            seconds).
    """

    deployment: str
    app: str
    offer: "Offer"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.offer import Offer

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
        from ..models.offer import Offer

        d = dict(src_dict)
        deployment = d.pop("deployment")

        app = d.pop("app")

        offer = Offer.from_dict(d.pop("offer"))

        app_offered = cls(
            deployment=deployment,
            app=app,
            offer=offer,
        )

        app_offered.additional_properties = d
        return app_offered

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
