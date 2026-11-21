from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..models.app_update_response_200_available_type_0_class import (
    AppUpdateResponse200AvailableType0Class,
)
from ..models.app_update_response_200_available_type_0_source import (
    AppUpdateResponse200AvailableType0Source,
)
from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.app_update_response_200_available_type_0_egress import (
        AppUpdateResponse200AvailableType0Egress,
    )
    from ..models.update_reason_type_4 import UpdateReasonType4
    from ..models.update_reason_type_3 import UpdateReasonType3
    from ..models.update_reason_type_0 import UpdateReasonType0
    from ..models.update_reason_type_5 import UpdateReasonType5
    from ..models.update_reason_type_2 import UpdateReasonType2
    from ..models.update_reason_type_1 import UpdateReasonType1


T = TypeVar("T", bound="AppUpdateResponse200AvailableType0")


@_attrs_define
class AppUpdateResponse200AvailableType0:
    """
    Attributes:
        package (str):
        version (str):
        source (AppUpdateResponse200AvailableType0Source):
        approvals_needed (int):
        class_ (Union[Unset, AppUpdateResponse200AvailableType0Class]):
        reasons (Union[Unset, list[Union['UpdateReasonType0', 'UpdateReasonType1', 'UpdateReasonType2',
            'UpdateReasonType3', 'UpdateReasonType4', 'UpdateReasonType5']]]):
        egress (Union[Unset, AppUpdateResponse200AvailableType0Egress]):
        state_version_changed (Union[Unset, bool]):
        new_methods (Union[Unset, list[str]]):
        removed_methods (Union[Unset, list[str]]):
        signer_changed (Union[Unset, bool]):
        summary (Union[Unset, str]):
        approved_by (Union[Unset, str]):
    """

    package: str
    version: str
    source: AppUpdateResponse200AvailableType0Source
    approvals_needed: int
    class_: Union[Unset, AppUpdateResponse200AvailableType0Class] = UNSET
    reasons: Union[
        Unset,
        list[
            Union[
                "UpdateReasonType0",
                "UpdateReasonType1",
                "UpdateReasonType2",
                "UpdateReasonType3",
                "UpdateReasonType4",
                "UpdateReasonType5",
            ]
        ],
    ] = UNSET
    egress: Union[Unset, "AppUpdateResponse200AvailableType0Egress"] = UNSET
    state_version_changed: Union[Unset, bool] = UNSET
    new_methods: Union[Unset, list[str]] = UNSET
    removed_methods: Union[Unset, list[str]] = UNSET
    signer_changed: Union[Unset, bool] = UNSET
    summary: Union[Unset, str] = UNSET
    approved_by: Union[Unset, str] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_update_response_200_available_type_0_egress import (
            AppUpdateResponse200AvailableType0Egress,
        )
        from ..models.update_reason_type_4 import UpdateReasonType4
        from ..models.update_reason_type_3 import UpdateReasonType3
        from ..models.update_reason_type_0 import UpdateReasonType0
        from ..models.update_reason_type_5 import UpdateReasonType5
        from ..models.update_reason_type_2 import UpdateReasonType2
        from ..models.update_reason_type_1 import UpdateReasonType1

        package = self.package

        version = self.version

        source = self.source.value

        approvals_needed = self.approvals_needed

        class_: Union[Unset, str] = UNSET
        if not isinstance(self.class_, Unset):
            class_ = self.class_.value

        reasons: Union[Unset, list[dict[str, Any]]] = UNSET
        if not isinstance(self.reasons, Unset):
            reasons = []
            for reasons_item_data in self.reasons:
                reasons_item: dict[str, Any]
                if isinstance(reasons_item_data, UpdateReasonType0):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType1):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType2):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType3):
                    reasons_item = reasons_item_data.to_dict()
                elif isinstance(reasons_item_data, UpdateReasonType4):
                    reasons_item = reasons_item_data.to_dict()
                else:
                    reasons_item = reasons_item_data.to_dict()

                reasons.append(reasons_item)

        egress: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.egress, Unset):
            egress = self.egress.to_dict()

        state_version_changed = self.state_version_changed

        new_methods: Union[Unset, list[str]] = UNSET
        if not isinstance(self.new_methods, Unset):
            new_methods = self.new_methods

        removed_methods: Union[Unset, list[str]] = UNSET
        if not isinstance(self.removed_methods, Unset):
            removed_methods = self.removed_methods

        signer_changed = self.signer_changed

        summary = self.summary

        approved_by = self.approved_by

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "package": package,
                "version": version,
                "source": source,
                "approvalsNeeded": approvals_needed,
            }
        )
        if class_ is not UNSET:
            field_dict["class"] = class_
        if reasons is not UNSET:
            field_dict["reasons"] = reasons
        if egress is not UNSET:
            field_dict["egress"] = egress
        if state_version_changed is not UNSET:
            field_dict["stateVersionChanged"] = state_version_changed
        if new_methods is not UNSET:
            field_dict["newMethods"] = new_methods
        if removed_methods is not UNSET:
            field_dict["removedMethods"] = removed_methods
        if signer_changed is not UNSET:
            field_dict["signerChanged"] = signer_changed
        if summary is not UNSET:
            field_dict["summary"] = summary
        if approved_by is not UNSET:
            field_dict["approvedBy"] = approved_by

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_update_response_200_available_type_0_egress import (
            AppUpdateResponse200AvailableType0Egress,
        )
        from ..models.update_reason_type_4 import UpdateReasonType4
        from ..models.update_reason_type_3 import UpdateReasonType3
        from ..models.update_reason_type_0 import UpdateReasonType0
        from ..models.update_reason_type_5 import UpdateReasonType5
        from ..models.update_reason_type_2 import UpdateReasonType2
        from ..models.update_reason_type_1 import UpdateReasonType1

        d = dict(src_dict)
        package = d.pop("package")

        version = d.pop("version")

        source = AppUpdateResponse200AvailableType0Source(d.pop("source"))

        approvals_needed = d.pop("approvalsNeeded")

        _class_ = d.pop("class", UNSET)
        class_: Union[Unset, AppUpdateResponse200AvailableType0Class]
        if isinstance(_class_, Unset):
            class_ = UNSET
        else:
            class_ = AppUpdateResponse200AvailableType0Class(_class_)

        reasons = []
        _reasons = d.pop("reasons", UNSET)
        for reasons_item_data in _reasons or []:

            def _parse_reasons_item(
                data: object,
            ) -> Union[
                "UpdateReasonType0",
                "UpdateReasonType1",
                "UpdateReasonType2",
                "UpdateReasonType3",
                "UpdateReasonType4",
                "UpdateReasonType5",
            ]:
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_0 = (
                        UpdateReasonType0.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_0
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_1 = (
                        UpdateReasonType1.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_1
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_2 = (
                        UpdateReasonType2.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_2
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_3 = (
                        UpdateReasonType3.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_3
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_update_reason_type_4 = (
                        UpdateReasonType4.from_dict(data)
                    )

                    return componentsschemas_update_reason_type_4
                except:  # noqa: E722
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_update_reason_type_5 = UpdateReasonType5.from_dict(
                    data
                )

                return componentsschemas_update_reason_type_5

            reasons_item = _parse_reasons_item(reasons_item_data)

            reasons.append(reasons_item)

        _egress = d.pop("egress", UNSET)
        egress: Union[Unset, AppUpdateResponse200AvailableType0Egress]
        if isinstance(_egress, Unset):
            egress = UNSET
        else:
            egress = AppUpdateResponse200AvailableType0Egress.from_dict(_egress)

        state_version_changed = d.pop("stateVersionChanged", UNSET)

        new_methods = cast(list[str], d.pop("newMethods", UNSET))

        removed_methods = cast(list[str], d.pop("removedMethods", UNSET))

        signer_changed = d.pop("signerChanged", UNSET)

        summary = d.pop("summary", UNSET)

        approved_by = d.pop("approvedBy", UNSET)

        app_update_response_200_available_type_0 = cls(
            package=package,
            version=version,
            source=source,
            approvals_needed=approvals_needed,
            class_=class_,
            reasons=reasons,
            egress=egress,
            state_version_changed=state_version_changed,
            new_methods=new_methods,
            removed_methods=removed_methods,
            signer_changed=signer_changed,
            summary=summary,
            approved_by=approved_by,
        )

        app_update_response_200_available_type_0.additional_properties = d
        return app_update_response_200_available_type_0

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
