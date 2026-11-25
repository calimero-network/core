from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_egress_response_200_egress import AppEgressResponse200Egress
from ..models.app_egress_response_200_off_by import AppEgressResponse200OffBy
from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.app_egress_response_200_members import AppEgressResponse200Members


T = TypeVar("T", bound="AppEgressResponse200")


@_attrs_define
class AppEgressResponse200:
    """
    Attributes:
        app (str):
        destinations (list[str]):
        members (AppEgressResponse200Members):
        egress (AppEgressResponse200Egress):
        off_by (Union[Unset, AppEgressResponse200OffBy]):
        summary (Union[Unset, str]):
    """

    app: str
    destinations: list[str]
    members: "AppEgressResponse200Members"
    egress: AppEgressResponse200Egress
    off_by: Union[Unset, AppEgressResponse200OffBy] = UNSET
    summary: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_egress_response_200_members import AppEgressResponse200Members

        app = self.app

        destinations = self.destinations

        members = self.members.to_dict()

        egress = self.egress.value

        off_by: Union[Unset, str] = UNSET
        if not isinstance(self.off_by, Unset):
            off_by = self.off_by.value

        summary = self.summary

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "destinations": destinations,
                "members": members,
                "egress": egress,
            }
        )
        if off_by is not UNSET:
            field_dict["offBy"] = off_by
        if summary is not UNSET:
            field_dict["summary"] = summary

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_egress_response_200_members import AppEgressResponse200Members

        d = dict(src_dict)
        app = d.pop("app")

        destinations = cast(list[str], d.pop("destinations"))

        members = AppEgressResponse200Members.from_dict(d.pop("members"))

        egress = AppEgressResponse200Egress(d.pop("egress"))

        _off_by = d.pop("offBy", UNSET)
        off_by: Union[Unset, AppEgressResponse200OffBy]
        if isinstance(_off_by, Unset):
            off_by = UNSET
        else:
            off_by = AppEgressResponse200OffBy(_off_by)

        summary = d.pop("summary", UNSET)

        app_egress_response_200 = cls(
            app=app,
            destinations=destinations,
            members=members,
            egress=egress,
            off_by=off_by,
            summary=summary,
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
