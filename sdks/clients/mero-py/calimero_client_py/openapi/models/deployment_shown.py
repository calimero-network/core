from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import Union

if TYPE_CHECKING:
    from ..models.deployment_shown_owner import DeploymentShownOwner
    from ..models.deployment_shown_transferred import DeploymentShownTransferred


T = TypeVar("T", bound="DeploymentShown")


@_attrs_define
class DeploymentShown:
    """The deployment an instance comes from, as its customers are shown it: names are not unique, so always with its id
    and its owner's kind and name (`null` once the owner is gone); once it changed hands, whom it came from and when
    (`transferred`, RFC 3339).

        Attributes:
            id (str):
            name (str):
            owner (DeploymentShownOwner):
            transferred (Union[Unset, DeploymentShownTransferred]):
    """

    id: str
    name: str
    owner: "DeploymentShownOwner"
    transferred: Union[Unset, "DeploymentShownTransferred"] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.deployment_shown_owner import DeploymentShownOwner
        from ..models.deployment_shown_transferred import DeploymentShownTransferred

        id = self.id

        name = self.name

        owner = self.owner.to_dict()

        transferred: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.transferred, Unset):
            transferred = self.transferred.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "name": name,
                "owner": owner,
            }
        )
        if transferred is not UNSET:
            field_dict["transferred"] = transferred

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.deployment_shown_owner import DeploymentShownOwner
        from ..models.deployment_shown_transferred import DeploymentShownTransferred

        d = dict(src_dict)
        id = d.pop("id")

        name = d.pop("name")

        owner = DeploymentShownOwner.from_dict(d.pop("owner"))

        _transferred = d.pop("transferred", UNSET)
        transferred: Union[Unset, DeploymentShownTransferred]
        if isinstance(_transferred, Unset):
            transferred = UNSET
        else:
            transferred = DeploymentShownTransferred.from_dict(_transferred)

        deployment_shown = cls(
            id=id,
            name=name,
            owner=owner,
            transferred=transferred,
        )

        deployment_shown.additional_properties = d
        return deployment_shown

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
