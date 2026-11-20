from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.app_egress_response_200_members import AppEgressResponse200Members


T = TypeVar("T", bound="AppEgressResponse200")


@_attrs_define
class AppEgressResponse200:
    """
    Attributes:
        app (str):
        egress (list[str]):
        sealed (bool):
        sealed_endpoints (list[str]):
        members (AppEgressResponse200Members):
    """

    app: str
    egress: list[str]
    sealed: bool
    sealed_endpoints: list[str]
    members: "AppEgressResponse200Members"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_egress_response_200_members import AppEgressResponse200Members

        app = self.app

        egress = self.egress

        sealed = self.sealed

        sealed_endpoints = self.sealed_endpoints

        members = self.members.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "egress": egress,
                "sealed": sealed,
                "sealedEndpoints": sealed_endpoints,
                "members": members,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_egress_response_200_members import AppEgressResponse200Members

        d = dict(src_dict)
        app = d.pop("app")

        egress = cast(list[str], d.pop("egress"))

        sealed = d.pop("sealed")

        sealed_endpoints = cast(list[str], d.pop("sealedEndpoints"))

        members = AppEgressResponse200Members.from_dict(d.pop("members"))

        app_egress_response_200 = cls(
            app=app,
            egress=egress,
            sealed=sealed,
            sealed_endpoints=sealed_endpoints,
            members=members,
        )

        app_egress_response_200.additional_properties = d
        return app_egress_response_200

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
