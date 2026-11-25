from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.egress_view_egress import EgressViewEgress
from ..models.egress_view_off_by import EgressViewOffBy
from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.egress_view_members import EgressViewMembers


T = TypeVar("T", bound="EgressView")


@_attrs_define
class EgressView:
    """`GET /apps/{app}/egress`: where the instance's unit may send data now (`destinations`, `members`), and whether its
    egress switch is on.

        Attributes:
            app (str):
            destinations (list[str]):
            members (EgressViewMembers):
            egress (EgressViewEgress):
            off_by (Union[Unset, EgressViewOffBy]):
            summary (Union[Unset, str]):
    """

    app: str
    destinations: list[str]
    members: "EgressViewMembers"
    egress: EgressViewEgress
    off_by: Union[Unset, EgressViewOffBy] = UNSET
    summary: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.egress_view_members import EgressViewMembers

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
        from ..models.egress_view_members import EgressViewMembers

        d = dict(src_dict)
        app = d.pop("app")

        destinations = cast(list[str], d.pop("destinations"))

        members = EgressViewMembers.from_dict(d.pop("members"))

        egress = EgressViewEgress(d.pop("egress"))

        _off_by = d.pop("offBy", UNSET)
        off_by: Union[Unset, EgressViewOffBy]
        if isinstance(_off_by, Unset):
            off_by = UNSET
        else:
            off_by = EgressViewOffBy(_off_by)

        summary = d.pop("summary", UNSET)

        egress_view = cls(
            app=app,
            destinations=destinations,
            members=members,
            egress=egress,
            off_by=off_by,
            summary=summary,
        )

        egress_view.additional_properties = d
        return egress_view

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
